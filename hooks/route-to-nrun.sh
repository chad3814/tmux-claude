#!/usr/bin/env bash
# route-to-nrun.sh — Claude Code PreToolUse hook.
#
# Transparently routes allowlisted long-lived commands (dev servers, docker,
# build tools) through `nrun` so they run in a dedicated tmux window while
# Claude still reads their output. Anything unrecognized runs unchanged.
#
# Fail-open: on ANY error, unrecognized input, or non-match, print nothing and
# exit 0 so the original command runs exactly as issued.

set -u

passthrough() { exit 0; }

# Dependencies. Without jq we cannot parse/emit JSON safely.
command -v jq >/dev/null 2>&1 || passthrough

input="$(cat)" || passthrough

tool_name="$(printf '%s' "$input" | jq -r '.tool_name // empty' 2>/dev/null)" || passthrough
[ "$tool_name" = "Bash" ] || passthrough

cmd="$(printf '%s' "$input" | jq -r '.tool_input.command // empty' 2>/dev/null)" || passthrough
[ -n "$cmd" ] || passthrough

# nrun must be installed; otherwise routing is impossible.
command -v nrun >/dev/null 2>&1 || passthrough

# --- reject command-substitution / pipes / redirects / subshells / sequences -
# (A leading `cd <dir> &&` is the only `&` we tolerate; handled just below.)
case "$cmd" in
    *'|'* | *';'* | *'<'* | *'>'* | *'('* | *')'* | *'`'*) passthrough ;;
esac

# --- strip a single leading `cd <dir> &&` ------------------------------------
cd_dir=""
if [[ "$cmd" =~ ^[[:space:]]*cd[[:space:]]+([^[:space:]&]+)[[:space:]]*\&\&[[:space:]]*(.+)$ ]]; then
    cd_dir="${BASH_REMATCH[1]}"
    cmd="${BASH_REMATCH[2]}"
fi

# --- strip leading VAR=value env assignments ---------------------------------
# Values are written UNQUOTED into the script (`export VAR=value`) so that
# `$VAR` references expand at run time. A value containing a quote or backslash
# can't be emitted safely unquoted, so bail to passthrough (fail-open) rather
# than write a broken script line.
env_assignments=()
while [[ "$cmd" =~ ^[[:space:]]*([A-Za-z_][A-Za-z0-9_]*=[^[:space:]&]*)[[:space:]]+(.+)$ ]]; do
    assignment="${BASH_REMATCH[1]}"
    rest="${BASH_REMATCH[2]}"
    case "$assignment" in
        *\'* | *\"* | *\\*) passthrough ;;
    esac
    env_assignments+=("$assignment")
    cmd="$rest"
done

# trim surrounding whitespace
cmd="${cmd#"${cmd%%[![:space:]]*}"}"
cmd="${cmd%"${cmd##*[![:space:]]}"}"

# any remaining `&` is a background/stray operator we do not handle
case "$cmd" in *'&'*) passthrough ;; esac
[ -n "$cmd" ] || passthrough

# --- tokenize ----------------------------------------------------------------
read -r -a toks <<< "$cmd"
[ "${#toks[@]}" -ge 1 ] || passthrough

# transparent `npx` prefix
if [ "${toks[0]}" = "npx" ] && [ "${#toks[@]}" -ge 2 ]; then
    eff=("${toks[@]:1}")
else
    eff=("${toks[@]}")
fi
eprog="${eff[0]}"

# recursion guard
[ "$eprog" = "nrun" ] && passthrough

contains() {
    local needle="$1"
    shift
    local x
    for x in "$@"; do [ "$x" = "$needle" ] && return 0; done
    return 1
}

# --- allowlist ---------------------------------------------------------------
# Membership (`contains`) is used instead of positional matching on purpose: the
# `dev` script can sit after flags (e.g. `pnpm --filter web dev`, and for npm
# `npm --workspace web run dev`), so a fixed position would miss real
# invocations. The only over-match is a nonsense ordering like `npm dev run`,
# which is harmless (it fails the same whether routed through nrun or not).
matched=0
is_docker_run=0
is_compose=0
case "$eprog" in
    pnpm) contains dev "${eff[@]:1}" && matched=1 ;;
    npm) contains run "${eff[@]:1}" && contains dev "${eff[@]:1}" && matched=1 ;;
    yarn) contains dev "${eff[@]:1}" && matched=1 ;;
    next) contains dev "${eff[@]:1}" && matched=1 ;;
    vite) matched=1 ;;
    make | cmake) matched=1 ;;
    configure | ./configure) matched=1 ;;
    docker)
        if [ "${eff[1]:-}" = "run" ]; then
            matched=1
            is_docker_run=1
        elif [ "${eff[1]:-}" = "compose" ] && [ "${eff[2]:-}" = "up" ]; then
            matched=1
            is_compose=1
        fi
        ;;
    docker-compose) [ "${eff[1]:-}" = "up" ] && {
        matched=1
        is_compose=1
    } ;;
esac
[ "$matched" = "1" ] || passthrough

# --- title (priority: --filter > program name; docker image added in Task 5) -
title=""
for ((i = 0; i < ${#eff[@]}; i++)); do
    case "${eff[i]}" in
        --filter | -F) title="${eff[i + 1]:-}"; break ;;
        --filter=*) title="${eff[i]#--filter=}"; break ;;
    esac
done
[ -z "$title" ] && title="$eprog"

docker_name=""
effective_cmd="$cmd"

if [ "$is_docker_run" = "1" ]; then
    # flags whose argument must be skipped when hunting for the image token
    valflags=" -p --publish -e --env --env-file -v --volume --mount --name -w --workdir --network --net -u --user --entrypoint -l --label -h --hostname --add-host --device --restart --expose --link -m --memory --cpus --gpus --platform --pull "
    args=("${eff[@]:2}")
    opts=()
    cmd_tail=()
    image=""
    has_name=""
    name_val=""
    seen_rm=0
    skip_next=0
    for ((j = 0; j < ${#args[@]}; j++)); do
        a="${args[j]}"
        if [ "$skip_next" = "1" ]; then
            skip_next=0
            opts+=("$a")
            continue
        fi
        if [ -n "$image" ]; then
            cmd_tail+=("$a")
            continue
        fi
        # Only exact -d/--detach is de-detached; a combined short cluster like
        # -dit is left as-is (best-effort), so it would still run detached.
        case "$a" in
            -d | --detach) ;;
            --rm)
                seen_rm=1
                opts+=("$a")
                ;;
            --name)
                has_name=1
                name_val="${args[j + 1]:-}"
                opts+=("$a")
                skip_next=1
                ;;
            --name=*)
                has_name=1
                name_val="${a#--name=}"
                opts+=("$a")
                ;;
            -*)
                opts+=("$a")
                [[ " $valflags " == *" $a "* ]] && skip_next=1
                ;;
            *) image="$a" ;;
        esac
    done
    img_base="${image##*/}"
    img_base="${img_base%%:*}"
    img_base="${img_base%%@*}"
    if [ -n "$has_name" ]; then
        docker_name="$name_val"
    else
        docker_name="${img_base:-docker}"
    fi
    rebuilt=("docker" "run")
    [ "${#opts[@]}" -gt 0 ] && rebuilt+=("${opts[@]}")
    [ -z "$has_name" ] && rebuilt+=("--name" "$docker_name")
    [ "$seen_rm" = "0" ] && rebuilt+=("--rm")
    rebuilt+=("$image")
    [ "${#cmd_tail[@]}" -gt 0 ] && rebuilt+=("${cmd_tail[@]}")
    effective_cmd="${rebuilt[*]}"
    [ -z "$title" ] || [ "$title" = "docker" ] && title="${img_base:-docker}"
elif [ "$is_compose" = "1" ]; then
    rebuilt=()
    for t in "${eff[@]}"; do
        case "$t" in
            -d | --detach) ;;
            *) rebuilt+=("$t") ;;
        esac
    done
    effective_cmd="${rebuilt[*]}"
    title="compose"
fi

# --- write the temp script (no nested quoting in the rewritten command) ------
script="$(mktemp "${TMPDIR:-/tmp}/nrun-script-XXXXXX.sh")" || passthrough
{
    printf '#!/usr/bin/env bash\n'
    [ -n "$cd_dir" ] && printf 'cd %q\n' "$cd_dir"
    for kv in ${env_assignments[@]+"${env_assignments[@]}"}; do
        printf 'export %s\n' "$kv"
    done
    printf 'exec %s\n' "$effective_cmd"
} >"$script" || passthrough
chmod +x "$script" 2>/dev/null || true

# --- build the nrun command --------------------------------------------------
nrun_cmd="nrun --title $(printf '%q' "$title")"
[ -n "$docker_name" ] && nrun_cmd+=" --docker-name $(printf '%q' "$docker_name")"
nrun_cmd+=" $(printf '%q' "$script")"

# --- emit updatedInput, preserving all original tool_input fields ------------
printf '%s' "$input" | jq -c --arg cmd "$nrun_cmd" \
    '{hookSpecificOutput: {hookEventName: "PreToolUse", updatedInput: (.tool_input + {command: $cmd, run_in_background: true})}}' \
    2>/dev/null || passthrough
