//! `deepdoc` — any document to clean Markdown, in one command.

mod args;
mod log;
mod manifest;
#[cfg(feature = "ocr")]
mod ocr;
mod run;

use clap::Parser;

use crate::args::Args;

fn main() {
    let args = Args::parse();
    if let Err(error) = args.check_ocr_support() {
        error.exit();
    }

    let code = match run::run(&args) {
        Ok(code) => code,
        Err(error) => {
            eprintln!("error: {error:#}");
            run::exit_code_for(&error)
        }
    };

    std::process::exit(code);
}
