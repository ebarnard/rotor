#![crate_name="rotor"]

extern crate netbuf;
extern crate mio;
#[macro_use] extern crate log;
extern crate memchr;

pub mod transports;
pub mod handler;
pub mod buffer_util;

pub use handler::{EventMachine, Handler, Scope, Config, EventSet, PollOpt, Evented};
