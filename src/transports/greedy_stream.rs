//! The simplest to use transport a `gready_stream`
//!
//! The transport agressively reads everything from input socket putting it
//! into the buffer. Similarly everything put into output buffer is just sent
//! back to the user.
//!
//! It's assumed that Protocol is able to keep up with the input rate. But if
//! it's not always the case you can always see input buffer size and drop
//! a connection. Currently you can't throttle neither reading end nor writer
//! (i.e. don't put everything to the output buffer at once)
//!
//! This is tradeoff to have super simple protocol and semantics. More
//! elaborate protocols will be implemented in the future.
//!
use std::io::{Read, Write, Error};
use std::marker::PhantomData;
use std::io::ErrorKind::{WouldBlock, Interrupted};

use netbuf::Buf;

use super::StreamSocket as Socket;
use super::accept::Init;

use {EventMachine, Scope, Config, EventSet, PollOpt, Evented};

impl<T> Socket for T where T: Read, T: Write, T: Evented {}

struct Stream<T, P, C> {
    sock: T,
    inbuf: Buf,
    outbuf: Buf,
    writable: bool,
    readable: bool,
    protocol: Option<P>,
    phantom: PhantomData<*const C>
}

unsafe impl<T: Socket+Send, P: Protocol<T, C>, C: Config> Send for Stream<T, P, C> {}

pub struct Transport<'a> {
    pub input: &'a mut Buf,
    pub output: &'a mut Buf,
}

/// This trait you should implement to handle the protocol. Only data_received
/// handler is required, everything else may be left as is.
pub trait Protocol<T, C>: 'static + Send + Sized
    where C: Config
{
    /// Returns new state machine in a state for new accepted connection
    fn accepted(conn: &mut T, ctx: &mut C::Context) -> Option<Self>;
    
    /// Some chunk of data has been received and placed into the buffer
    ///
    /// It's edge-triggered so be sure to read everything useful. But you
    /// can leave half-received packets in the buffer
    fn data_received(self, transport: &mut Transport, ctx: &mut C::Context)
        -> Option<Self>;

    /// Eof received. State machine will shutdown unconditionally
    fn eof_received(self, _ctx: &mut C::Context) {}

    /// Fatal error on connection happened, you may process error somehow, but
    /// statemachine will be destroyed anyway (note you receive self)
    ///
    /// Default action is to log error on the info level
    fn error_happened(self, e: Error, _ctx: &mut C::Context) {
        info!("Error when handling connection: {}", e);
    }
}

impl<T, P, C> Init<T, C> for Stream<T, P, C>
    where T: Socket + Send + 'static,
          P: Protocol<T, C>,
          C: Config
{
    fn accept(mut conn: T, context: &mut C::Context, _scope: &mut Scope<C>) -> Option<Self> {
        Protocol::accepted(&mut conn, context).map(|protocol|
            Stream {
                sock: conn,
                inbuf: Buf::new(),
                outbuf: Buf::new(),
                readable: false,
                writable: true,   // Accepted socket is immediately writable
                protocol: Some(protocol),
                phantom: PhantomData
            }
        )
    }
}

impl<T, P, C> EventMachine<C> for Stream<T, P, C>
    where T: Socket + Send + 'static,
          P: Protocol<T, C>,
          C: Config
{
    fn ready(&mut self, evset: EventSet, context: &mut C::Context, _scope: &mut Scope<C>)
        -> Option<()>
    {
        if let Some(mut protocol) = self.protocol.take() {
            if evset.is_writable() && self.outbuf.len() > 0 {
                self.writable = true;
                while self.outbuf.len() > 0 {
                    match self.outbuf.write_to(&mut self.sock) {
                        Ok(0) => { // Connection closed
                            protocol.eof_received(context);
                            return None;
                        }
                        Ok(_) => {}  // May notify application
                        Err(ref e) if e.kind() == WouldBlock => {
                            self.writable = false;
                            break;
                        }
                        Err(ref e) if e.kind() == Interrupted =>  { continue; }
                        Err(e) => {
                            protocol.error_happened(e, context);
                            return None;
                        }
                    }
                }
            }
            if evset.is_readable() {
                self.readable = true;
                loop {
                    match self.inbuf.read_from(&mut self.sock) {
                        Ok(0) => { // Connection closed
                            protocol.eof_received(context);
                            return None;
                        }
                        Ok(_) => {
                            protocol = match protocol.data_received(&mut Transport {
                                input: &mut self.inbuf,
                                output: &mut self.outbuf,
                            }, context) {
                                Some(protocol) => protocol,
                                None => return None,
                            };
                        }
                        Err(ref e) if e.kind() == WouldBlock => {
                            self.readable = false;
                            break;
                        }
                        Err(ref e) if e.kind() == Interrupted =>  { continue; }
                        Err(e) => {
                            protocol.error_happened(e, context);
                            return None;
                        }
                    }
                }
            }
            if self.writable && self.outbuf.len() > 0 {
                while self.outbuf.len() > 0 {
                    match self.outbuf.write_to(&mut self.sock) {
                        Ok(0) => { // Connection closed
                            protocol.eof_received(context);
                            return None;
                        }
                        Ok(_) => {}  // May notify application
                        Err(ref e) if e.kind() == WouldBlock => {
                            self.writable = false;
                            break;
                        }
                        Err(ref e) if e.kind() == Interrupted =>  { continue; }
                        Err(e) => {
                            protocol.error_happened(e, context);
                            return None;
                        }
                    }
                }
            }
            self.protocol = Some(protocol);
            Some(())
        } else {
            None
        }
    }

    fn register(&mut self, scope: &mut Scope<C>) -> Result<(), Error> {
        scope.register(&self.sock, EventSet::all(), PollOpt::edge())
    }
}
