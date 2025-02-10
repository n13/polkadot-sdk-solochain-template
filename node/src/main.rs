//! Substrate Node Template CLI library.
#![warn(missing_docs)]
#![feature(type_alias_impl_trait)]

mod benchmarking;
mod chain_spec;
mod cli;
mod command;
mod rpc;
mod service;

fn main() -> sc_cli::Result<()> {
	command::run()
}
