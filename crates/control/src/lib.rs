//! The daemon's own IPC: a line-delimited JSON request/response protocol over a unix
//! socket, plus the client half.
//!
//! The socket path is supplied by the caller, so the program's name has no second
//! definition point here.

pub mod client;
pub mod protocol;
pub mod server;

pub use client::{Client, ClientError};
pub use protocol::{
    BlurSnapshot, GpuSnapshot, IndexSnapshot, Micros, OutputSnapshot, PROTOCOL_VERSION, Request,
    Response, ScrollSnapshot, StateSnapshot, Tween,
};
pub use server::{Handler, Server, ServerError};
