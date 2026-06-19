//! Drives the `claude` CLI over the stream-json + control protocol.

pub mod codec;
pub mod spawn;
pub mod mcp;
pub mod control;
pub mod process;
pub mod pool;
pub mod factory;
