//! The daemon's own IPC: a line-delimited JSON request/response protocol over a unix
//! socket, plus the client half.
//!
//! A connection that subscribes stops answering and becomes a one-way stream of events,
//! which is the only arrangement in which the two kinds of line cannot be confused.
//!
//! The socket path is supplied by the caller, so the program's name has no second
//! definition point here.

pub mod client;
pub mod protocol;
pub mod server;

pub use client::{Client, ClientError, Subscription};
pub use protocol::{
    BlurSnapshot, ChannelSnapshot, Event, GpuSnapshot, Micros, OutputSnapshot, PROTOCOL_VERSION,
    Property, Request, Response, ScrollSnapshot, StateSnapshot, Tween, Values,
};
pub use server::{Handler, Server, ServerError, Subscriber, Subscribers};
