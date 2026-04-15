use anyhow::Result;
use clap::Parser;

fn main() -> Result<()> {
    docbake::run(docbake::cli::Cli::parse())
}
