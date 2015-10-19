use std::io::Error;
use std::marker::PhantomData;

use mio::TryAccept;

use {EventMachine, Scope, Config, EventSet, PollOpt, Evented};
use handler::Abort::MachineAddError;

pub struct Serve<A, M, C>(A, PhantomData<(*const M, *const C)>);

unsafe impl<A, M, C> Send for Serve<A, M, C>
    where M: Init<A::Output, C>,
          A: Evented + TryAccept + Send + 'static,
          C: Config
{}

pub trait Init<S, C>: EventMachine<C> + Sized
    where C: Config
{
    fn accept(conn: S, context: &mut C::Context, scope: &mut Scope<C>) -> Option<Self>;
}

impl<A, M, C> EventMachine<C> for Serve<A, M, C>
    where A: Evented + TryAccept + Send + 'static,
          M: Init<A::Output, C>,
          C: Config
{
    fn ready(&mut self, evset: EventSet, context: &mut C::Context, scope: &mut Scope<C>)
        -> Option<()>
    {
        if !evset.is_readable() {
            return Some(())
        }

        match self.0.accept() {
            Ok(Some(child)) => {
                <M as Init<_, _>>::accept(child, context, scope)
                    .ok_or(())
                    .and_then(|conm|
                        scope.add_machine(conm)
                        .map_err(|mut child| child.abort(MachineAddError, context, scope)))
                    .ok();
            }
            Ok(None) => {}
            Err(e) => {
                error!("Error on socket accept: {}", e);
            }
        }

        Some(())
    }

    fn register(&mut self, scope: &mut Scope<C>) -> Result<(), Error> {
        scope.register(&self.0, EventSet::readable(), PollOpt::level())
    }
}

impl<A, S, M, C> Serve<A, M, C>
    where M: Init<S, C>,
          A: Evented + TryAccept<Output=S> + Send + 'static,
          C: Config
{
    pub fn new(sock: A) -> Self {
        Serve(sock, PhantomData)
    }
}
