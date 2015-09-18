use mio::{self, EventLoop, Sender};
use mio::util::Slab;
use std::io::Error;
use std::time::Duration;
use std::marker::PhantomData;

pub use mio::{Evented, EventSet, PollOpt};

pub trait Config: 'static {
    type Context;
    type Message: Send;
    type Timeout;
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub enum Abort {
    RegisterFailed,
    MachineAddError
}

pub struct Message<C>(MessageInner<C>)
    where C: Config;

enum MessageInner<C>
    where C: Config
{
    RegisterMachine(Token),
    Phantom(PhantomData<C::Message>)
}

pub struct Timeout<C>
    where C: Config
{
    token: Token,
    timeout: C::Timeout
}

pub struct Scope<'a, C>
    where C: 'a + Config
{
    channel: &'a Sender<Message<C>>,
    eloop: &'a mut EventLoop<Handler<C>>,
    slab: &'a mut Slab<EventMachineSlot<C>>,
    counter_next: &'a mut u64,
    token: Token,
}

impl<'a, C> Scope<'a, C>
    where C: Config 
{
    pub fn add_machine(&mut self, machine: Box<EventMachine<C>>) -> Result<Token, Box<EventMachine<C>>> {
        use self::MessageInner::*;

        let slot = EventMachineSlot {
            machine: machine,
            counter: *self.counter_next
        };
        *self.counter_next += 1;

        self.slab.insert(slot).and_then(|mio_token| {
            let token = Token::from_mio(mio_token);

            match self.send_message(RegisterMachine(token)) {
                Ok(()) => Ok(token),
                Err(RegisterMachine(_)) => {
                    Err(self.slab.remove(mio_token).expect("This should not happen."))
                },
                _ => unreachable!()
            }
        }).map_err(|slot| slot.machine)
    }

    pub fn add_timeout(&mut self, delay: Duration, t: Timeout<C>)
        -> Result<mio::Timeout, mio::TimerError>
    {
        unimplemented!();
        // TODO: Use std duration
        //self.eloop.timeout_ms(t, delay)
    }

    pub fn clear_timeout(&mut self, timeout: mio::Timeout) -> bool {
        self.eloop.clear_timeout(timeout)
    }

    pub fn register<E: ?Sized>(&mut self, io: &E, interest: EventSet, opt: PollOpt)
        -> Result<(), Error>
        where E: Evented
    {
        self.eloop.register(io, self.token.mio_token, interest, opt)
    }

    fn send_message(&mut self, m: MessageInner<C>) -> Result<(), MessageInner<C>> {
        use mio::NotifyError::*;
        match self.channel.send(Message(m)) {
            Ok(()) => Ok(()),
            Err(Io(e)) => {
                // We would probably do something better here, but mio doesn't
                // give us a message. But anyway it's probably never happen
                panic!("Io error when sending notify: {}", e);
            }
            Err(Full(m)) => Err(m.0),
            Err(Closed(_)) => {
                // It should never happen because we usually send from the
                // inside of a main loop
                panic!("Sending to closed channel. Main loop is already shut \
                    down");
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Token {
    mio_token: mio::Token,
    counter: Option<u64>
}

impl Token {
    fn from_mio(mio_token: mio::Token) -> Token {
        Token {
            mio_token: mio_token,
            counter: None
        }
    }

    fn counter_eq(&self, other_counter: u64) -> bool {
        match self.counter {
            Some(counter) => counter == other_counter,
            None => true
        }
    }
}

pub struct EventMachineSlot<C>
    where C: Config
{
    machine: Box<EventMachine<C>>,
    counter: u64
}

pub struct Handler<C>
    where C: Config
{
    slab: Slab<EventMachineSlot<C>>,
    context: C::Context,
    channel: Sender<Message<C>>,
    counter_next: u64
}

pub trait EventMachine<C>: 'static + Send
    where C: Config
{
    /// Socket readiness notification
    fn ready(&mut self, events: EventSet, ctx: &mut C::Context, scope: &mut Scope<C>) -> Option<()>;

    /// Gives socket a chance to register in event loop
    fn register(&mut self, scope: &mut Scope<C>) -> Result<(), Error>;

    fn shutdown(&mut self, _ctx: &mut C::Context, _scope: &mut Scope<C>) -> Option<()> {
        // Shutdown immediately by default
        None
    }

    fn timeout(&mut self, _timeout: C::Timeout, _ctx: &mut C::Context, _scope: &mut Scope<C>) -> Option<()> {
        // Ignore timeouts by default
        Some(())
    }

    fn notify(&mut self, _msg: C::Message, _ctx: &mut C::Context, _scope: &mut Scope<C>) -> Option<()> {
        // Ignore notifications by default
        Some(())
    }

    /// Abnormal termination of event machine
    fn abort(&mut self, reason: Abort, _ctx: &mut C::Context, _scope: &mut Scope<C>) {
        // TODO(tailhook) use Display instead of Debug
        error!("Connection aborted: {:?}", reason);
    }
}

impl<C> Handler<C>
    where C: Config
{
    pub fn new(context: C::Context, eloop: &mut EventLoop<Handler<C>>) -> Handler<C> {
        // TODO(tailhook) create default config from the ulimit data instead
        // of using real defaults
        Handler {
            slab: Slab::new(4096),
            context: context,
            channel: eloop.channel(),
            counter_next: 0
        }
    }
}

impl<C> mio::Handler for Handler<C>
    where C: Config
{
    type Message = Message<C>;
    type Timeout = Timeout<C>;

    // TODO: Wrap these in a try/catch block so one error doesn't take down everything
    fn ready<'x>(&mut self, eloop: &'x mut EventLoop<Self>,
        token: mio::Token, events: EventSet)
    {
        self.with_machine(eloop, Token::from_mio(token), |fsm, ctx, scope|
            fsm.ready(events, ctx, scope)
        ).ok(); // Spurious events are ok in mio*/
    }

    fn notify(&mut self, eloop: &mut EventLoop<Self>, msg: Self::Message) {
        use self::MessageInner::*;
        match msg.0 {
            RegisterMachine(token) => {
                self.with_machine(eloop, token, |fsm, ctx, scope| {
                    match fsm.register(scope) {
                        Ok(()) => Some(()),
                        Err(_) => {
                            fsm.abort(Abort::RegisterFailed,
                                ctx, scope);
                            None
                        }
                    }
                }).ok(); // The machine may have already been removed
            },
            _ => unimplemented!()
        }
    }
}

impl<C> Handler<C>
    where C: Config
{
    fn with_machine<F>(&mut self, eloop: &mut EventLoop<Self>, token: Token, f: F) -> Result<(), ()>
        where F: FnOnce(&mut EventMachine<C>, &mut C::Context, &mut Scope<C>) -> Option<()>
    {
        let channel = &self.channel;
        let ctx = &mut self.context;
        let counter_next = &mut self.counter_next;
        self.slab.replace_with(token.mio_token, |mut slot, slab| {
            if token.counter_eq(slot.counter) {
                let ref mut scope = Scope {
                    eloop: eloop,
                    channel: channel,
                    slab: slab,
                    token: token,
                    counter_next: counter_next
                };
                f(&mut *slot.machine, ctx, scope).map(|()| slot)
            } else {
                // Token refers to a machine that has been removed
                Some(slot)
            }
        })
    }
}
