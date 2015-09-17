use std::io::Error;
use std::marker::PhantomData;

use mio::TryAccept;
use mio::{Token, EventSet, Handler, PollOpt, Evented};
use mio::{Timeout, TimerError};

use {BaseMachine, EventMachine, Scope};
use handler::Abort::MachineAddError;

pub enum Serve<A, M, C>
    where A: Evented + TryAccept + Send,
          M: Init<A::Output, C>,

{
    Accept(A, PhantomData<*const C>),
    Connection(M),
}

unsafe impl<A, M, C> Send for Serve<A, M, C>
    where M: Init<A::Output, C>,
          A: Evented + TryAccept + Send
{}

pub trait Init<T, C>: EventMachine<C> {
    fn accept<S>(conn: T, context: &mut C, scope: &mut S)
        -> Self
        where S: Scope<Self>;
}

struct ScopeProxy<'a, S: 'a, A, C>(&'a mut S, PhantomData<*const (A, C)>);

impl<'a, M, S, A, C> Scope<M> for ScopeProxy<'a, S, A, C>
    where S: Scope<Serve<A, M, C>> + 'a,
          A: Evented + TryAccept + Send,
          M: Init<A::Output, C>
{
    fn async_add_machine(&mut self, m: M) -> Result<Token, M> {
        self.0.async_add_machine(Serve::Connection(m))
        .map_err(|x| if let Serve::Connection(c) = x {
            c
        } else {
            unreachable!();
        })
    }
    fn add_timeout_ms(&mut self, delay: u64, t: M::Timeout)
        -> Result<Timeout, TimerError>
    {
        self.0.add_timeout_ms(delay, t)
    }
    fn clear_timeout(&mut self, timeout: Timeout) -> bool {
        self.0.clear_timeout(timeout)
    }
    fn register<E: ?Sized>(&mut self, io: &E, interest: EventSet, opt: PollOpt)
        -> Result<(), Error>
        where E: Evented
    {
        self.0.register(io, interest, opt)
    }
}
impl<A, M, C> BaseMachine for Serve<A, M, C>
    where A: Evented + TryAccept + Send,
          M: Init<A::Output, C>
{
    type Timeout = M::Timeout;
}

impl<A, M, C> EventMachine<C> for Serve<A, M, C>
    where A: Evented + TryAccept + Send,
          M: Init<A::Output, C>
{
    fn ready<S>(self, evset: EventSet, context: &mut C, scope: &mut S)
        -> Option<Self>
        where S: Scope<Self>
    {
        use self::Serve::*;
        match self {
            Accept(sock, _) => {
                match sock.accept() {
                    Ok(Some(child)) => {
                        let conm: M = <M as Init<_, _>>::accept(child, context,
                            &mut ScopeProxy(scope, PhantomData));
                        let conn: Serve<A, M, C> = Connection(conm);
                        scope.async_add_machine(conn)
                        .map_err(|child|
                            child.abort(MachineAddError, context, scope))
                        .ok();
                    }
                    Ok(None) => {}
                    Err(e) => {
                        error!("Error on socket accept: {}", e);
                    }
                }
                Some(Accept(sock, PhantomData))
            }
            Connection(c) => c.ready(evset, context,
                &mut ScopeProxy(scope, PhantomData))
                .map(Connection),
        }
    }

    fn register<Sc>(&mut self, scope: &mut Sc)
        -> Result<(), Error>
        where Sc: Scope<Self>
    {
        use self::Serve::*;
        match self {
            &mut Accept(ref mut s, _)
            => scope.register(s, EventSet::readable(), PollOpt::level()),
            &mut Connection(ref mut c)
            => c.register(&mut ScopeProxy(scope, PhantomData)),
        }
    }
}

impl<A, T, M, C> Serve<A, M, C>
    where M: Init<T, C>,
          A: Evented + TryAccept<Output=T> + Send,
{
    pub fn new(sock: A) -> Self {
        Serve::Accept(sock, PhantomData)
    }
}
