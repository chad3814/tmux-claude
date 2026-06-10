use clap::Parser;
use nrun::cli::Config;

fn main() {
    let cfg = Config::parse();
    match nrun::run(&cfg) {
        Ok(code) => std::process::exit(code),
        Err(e) => {
            eprintln!("nrun: {e}");
            std::process::exit(1);
        }
    }
}
