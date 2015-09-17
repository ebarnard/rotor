use std::io::Error;
use std::usize;

use mio::{self, EventLoop, Token, EventSet, Evented, PollOpt};
use mio::util::Slab;
use mio::{Sender, Timeout, TimerError};

use {Scope, BaseMachine};

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Abort {
    RegisterFailed,
    MachineAddError
}

pub enum Notify {
    RegisterMachine(Token)
}

struct RootScope<'a, M, C>
    where M: 'a + EventMachine<C>,
          C: 'a
{
    channel: &'a Sender<Notify>,
    eloop: &'a mut EventLoop<Handler<M, C>>,
    slab: &'a mut Slab<M>,
    token: Token,
}

pub struct Handler<M, C> {
    slab: Slab<M>,
    context: C,
    channel: Sender<Notify>,
}

pub trait EventMachine<C>: BaseMachine {
    /// Socket readiness notification
    fn ready<S>(self, events: EventSet, context: &mut C, scope: &mut S)
        -> Option<Self>
        where S: Scope<Self>;

    /// Gives socket a chance to register in event loop
    fn register<S>(&mut self, scope: &mut S)
        -> Result<(), Error>
        where S: Scope<Self>;

    fn shutdown<S>(&mut self, _context: &mut C, _scope: &mut S)
        where S: Scope<Self>
    {
        // Ignore shutdown requests by default
    }

    fn timeout<S>(&mut self, _timeout: Self::Timeout, _context: &mut C, _scope: &mut S)
        where S: Scope<Self>
    {
        // Ignore timeouts by default
    }

    /// Abnormal termination of event machine
    fn abort<S>(self, reason: Abort, _context: &mut C, _scope: &mut S)
        where S: Scope<Self>
    {
        // TODO(tailhook) use Display instead of Debug
        error!("Connection aborted: {:?}", reason);
    }
}

impl<M, C> Handler<M, C>
    where M: EventMachine<C>
{
    pub fn new(context: C, eloop: &mut EventLoop<Handler<M, C>>)
        -> Handler<M, C>
    {
        // TODO(tailhook) create default config from the ulimit data instead
        // of using real defaults
        Handler {
            slab: Slab::new(4096),
            context: context,
            channel: eloop.channel(),
        }
    }
}

impl<'a, M, C> Scope<M> for RootScope<'a, M, C>
    where M: EventMachine<C>
{
    fn async_add_machine(&mut self, m: M) -> Result<Token, M> {
        use self::Notify::*;
        self.slab.insert(m).and_then(|tok| {
            match self.send_message(RegisterMachine(tok)) {
                Ok(()) => Ok(tok),
                Err(RegisterMachine(tok)) => {
                    Err(self.slab.remove(tok).expect("This should not happen."))
                }
            }
        })
    }
    fn add_timeout_ms(&mut self, delay: u64, t: M::Timeout)
        -> Result<Timeout, TimerError>
    {
        self.eloop.timeout_ms(t, delay)
    }
    fn clear_timeout(&mut self, timeout: Timeout) -> bool {
        self.eloop.clear_timeout(timeout)
    }
    fn register<E: ?Sized>(&mut self, io: &E, interest: EventSet, opt: PollOpt)
        -> Result<(), Error>
        where E: Evented
    {
        self.eloop.register(io, self.token, interest, opt)
    }
}

impl<'a, M, C> RootScope<'a, M, C>
    where M: EventMachine<C>
{
    fn send_message(&mut self, m: Notify) ->
        Result<(), Notify>
    {
        use mio::NotifyError::*;
        match self.channel.send(m) {
            Ok(()) => Ok(()),
            Err(Io(e)) => {
                // We would probably do something better here, but mio doesn't
                // give us a message. But anyway it's probably never happen
                panic!("Io error when sending notify: {}", e);
            }
            Err(Full(m)) => Err(m),
            Err(Closed(_)) => {
                // It should never happen because we usually send from the
                // inside of a main loop
                panic!("Sending to closed channel. Main loop is already shut \
                    down");
            }
        }
    }
}

impl<'a, M, C> mio::Handler for Handler<M, C>
    where M: EventMachine<C>
{
    type Message = Notify;
    type Timeout = M::Timeout;
    fn ready<'x>(&mut self, eloop: &'x mut EventLoop<Self>,
        token: Token, events: EventSet)
    {
        let channel = &self.channel;
        let ctx = &mut self.context;
        self.slab.replace_with(token, |fsm, slab| {
            let ref mut scope = RootScope {
                eloop: eloop,
                channel: channel,
                slab: slab,
                token: token,
            };
            fsm.ready(events, ctx, scope)
        }).ok();  // Spurious events are ok in mio*/
    }

    fn notify(&mut self, eloop: &mut EventLoop<Self>, msg: Self::Message) {
        use self::Notify::*;
        match msg {
            RegisterMachine(tok) => {
                let channel = &self.channel;
                let ctx = &mut self.context;
                self.slab.replace_with(tok, |mut fsm, slab| {
                    let ref mut scope = RootScope {
                        eloop: eloop,
                        channel: channel,
                        slab: slab,
                        token: tok,
                    };
                    match fsm.register(scope) {
                        Ok(()) => Some(fsm),
                        Err(_) => {
                            fsm.abort(Abort::RegisterFailed,
                                ctx, scope);
                            None
                        }
                    }
                }).ok(); // The machine may have already been removed
            }
        }
    }
}

