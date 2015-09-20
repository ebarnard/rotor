#![crate_name="rotor"]

extern crate netbuf;
extern crate mio;
#[macro_use] extern crate log;
extern crate memchr;

pub mod transports;
pub mod handler;
pub mod buffer_util;

pub use handler::{EventMachine, Handler, Scope, Config, EventSet, PollOpt, Evented};


struct PhantomSend<C>(::std::marker::PhantomData<*const C>);
unsafe impl<C> Send for PhantomSend<C> {}
impl<C> PhantomSend<C> {
	pub fn new() -> PhantomSend<C> {
		PhantomSend(::std::marker::PhantomData)
	}
}