use mio;
use netbuf::Buf;
use std::io;
use std::io::ErrorKind::Interrupted;
use std::error::Error;
use std::net::SocketAddr;
use PhantomSend;
use std::collections::VecDeque;

use {EventMachine, Config, PollOpt, Scope, EventSet};

const UDP_MAX_SIZE: usize = 64 * 1024;

pub struct Socket<P, C>
	where P: Protocol<C>,
		  C: Config
{
	sock: mio::udp::UdpSocket,
	recv_buf: Box<[u8; UDP_MAX_SIZE]>,
	send_queue: VecDeque<(SocketAddr, Buf)>,
	protocol: Option<P>,
	phantom: PhantomSend<C::Context>
}

pub struct Packet<'a> {
	pub source: SocketAddr,
	pub data: &'a [u8],
}

pub struct Transport<'a> {
    send_queue: &'a mut VecDeque<(SocketAddr, Buf)>
}

impl<'a> Transport<'a> {
	pub fn send(&mut self, target: SocketAddr, buf: Buf) {
		self.send_queue.push_back((target, buf))
	}
}

pub trait Protocol<C> : 'static + Send + Sized
	where C: Config
{
    /// A datagram has been received
    fn packet_received(self, packet: Packet, transport: &mut Transport, ctx: &mut C::Context)
        -> Option<Self>;

    /// Fatal error on connection happened, you may process error somehow, but
    /// statemachine will be destroyed anyway (note you receive self)
    ///
    /// Default action is to log error on the info level
    fn error_happened(self, err: io::Error, _ctx: &mut C::Context) {
        info!("Error when handling connection: {}", err);
    }
}

impl<P, C> Socket<P, C>
    where P: Protocol<C>,
          C: Config
{
	pub fn new(sock: mio::udp::UdpSocket, protocol: P) -> Socket<P, C> {
		Socket {
			sock: sock,
			recv_buf: Box::new([0; UDP_MAX_SIZE]),
			send_queue: VecDeque::new(),
			protocol: Some(protocol),
			phantom: PhantomSend::new()
		}
	}
}

impl<P, C> EventMachine<C> for Socket<P, C>
    where P: Protocol<C>,
          C: Config
{
    fn ready(&mut self, evset: EventSet, ctx: &mut C::Context, _scope: &mut Scope<C>)
        -> Option<()>
    {
        if let Some(mut protocol) = self.protocol.take() {
            if evset.is_readable() {
                //readable = true;
                loop {
                	match self.sock.recv_from(&mut self.recv_buf[..]) {
                		Ok(Some((len, source))) => {
                			let pkt = Packet {
                				source: source,
                				data: &self.recv_buf[..len]
                			};
                			let tx = &mut Transport {
                				send_queue: &mut self.send_queue
                			};
                			protocol = match protocol.packet_received(pkt, tx, ctx) {
                				Some(protocol) => protocol,
                				None => return None,
                			};
                		},
                		Ok(None) => {
                			//readable = false;
                			break;
                		},
                		Err(ref e) if e.kind() == Interrupted => { continue; },
                		Err(e) => {
                			protocol.error_happened(e, ctx);
                			return None
                		}
                	}
                }
            }
            if evset.is_writable() && self.send_queue.len() > 0 {
                //writable = true;
                while let Some((target, buf)) = self.send_queue.pop_front() {
                	match self.sock.send_to(&buf[..], &target) {
                		Ok(Some(len)) => { /* TODO: Deal with not all written situation */ },
                		Ok(None) => {
                			//writable = false;
                			self.send_queue.push_front((target, buf));
                			break;
                		},
                		Err(ref e) if e.kind() == Interrupted => {
                			self.send_queue.push_front((target, buf));
                			continue;
                		},
                		Err(e) => {
                			self.send_queue.push_front((target, buf));
                			protocol.error_happened(e, ctx);
                			return None
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

    fn register(&mut self, scope: &mut Scope<C>) -> io::Result<()> {
        scope.register(&self.sock, EventSet::all(), PollOpt::level())
    }
}