use clap::Parser;
use rusttemp::cli::{Cli, Command};
use rusttemp::commands;

fn main() {
    let mut cli = Cli::parse();
    match cli.command.take() {
        Some(Command::Cpu(args)) => commands::finish("rusttemp cpu", commands::cpu::run(args)),
        Some(Command::Gpu(args)) => commands::finish("rusttemp gpu", commands::gpu::run(args)),
        None => commands::finish("rusttemp", cli.into_config().and_then(rusttemp::app::run)),
    }
}
