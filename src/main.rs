use clap::Parser;
use rusttemp::cli::{Cli, Command};
use rusttemp::commands;

fn main() {
    // `rusttemp` on its own is the live CPU report, refreshed every second:
    // exactly `rusttemp cpu --watch`.
    if std::env::args_os().len() <= 1 {
        let args = rusttemp::commands::cpu::CpuArgs::parse_from(["cpu", "--watch"]);
        commands::finish("rusttemp", commands::cpu::run(args));
    }
    let mut cli = Cli::parse();
    match cli.command.take() {
        Some(Command::Cpu(args)) => commands::finish("rusttemp cpu", commands::cpu::run(args)),
        Some(Command::Gpu(args)) => commands::finish("rusttemp gpu", commands::gpu::run(args)),
        None => commands::finish("rusttemp", cli.into_config().and_then(rusttemp::app::run)),
    }
}
