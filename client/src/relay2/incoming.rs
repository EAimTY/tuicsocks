use super::{
    stream::{IncomingDatagrams, IncomingUniStreams, RecvStream},
    Address, Connection, UdpRelayMode,
};
use bytes::Bytes;
use futures_util::StreamExt;
use quinn::ConnectionError;
use std::{
    io::{Error, ErrorKind, Result},
    sync::Arc,
};
use tokio::{
    io::AsyncReadExt,
    sync::oneshot::{self, Receiver as OneshotReceiver, Sender as OneshotSender},
};
use tuic_protocol::Command as TuicCommand;

pub async fn listen_incoming(
    mut next_incoming_rx: UdpRelayMode<Receiver<IncomingDatagrams>, Receiver<IncomingUniStreams>>,
) {
    loop {
        let (conn, incoming);
        (conn, incoming, next_incoming_rx) = match next_incoming_rx {
            UdpRelayMode::Native(incoming_rx) => {
                let (conn, incoming, next_incoming_rx) = incoming_rx.next().await;
                (
                    conn,
                    UdpRelayMode::Native(incoming),
                    UdpRelayMode::Native(next_incoming_rx),
                )
            }
            UdpRelayMode::Quic(incoming_rx) => {
                let (conn, incoming, next_incoming_rx) = incoming_rx.next().await;
                (
                    conn,
                    UdpRelayMode::Quic(incoming),
                    UdpRelayMode::Quic(next_incoming_rx),
                )
            }
        };

        let err = match incoming {
            UdpRelayMode::Native(mut incoming) => loop {
                let pkt = match incoming.next().await {
                    Some(Ok(pkt)) => pkt,
                    Some(Err(err)) => break err,
                    None => break ConnectionError::LocallyClosed,
                };

                // process datagram
                tokio::spawn(conn.clone().process_incoming_datagram(pkt));
            },
            UdpRelayMode::Quic(mut uni) => loop {
                let recv = match uni.next().await {
                    Some(Ok(recv)) => recv,
                    Some(Err(err)) => break err,
                    None => break ConnectionError::LocallyClosed,
                };

                // process uni stream
                tokio::spawn(conn.clone().process_incoming_uni_stream(recv));
            },
        };

        match err {
            ConnectionError::LocallyClosed | ConnectionError::TimedOut => {}
            err => log::error!("[relay] [connection] {err}"),
        }

        conn.set_closed();
    }
}

impl Connection {
    async fn process_incoming_datagram(self, pkt: Bytes) {
        async fn parse_header(pkt: Bytes) -> Result<(u32, Bytes, Address)> {
            let cmd = TuicCommand::read_from(&mut pkt.as_ref()).await.unwrap(); // TODO: handle error
            let cmd_len = cmd.serialized_len();

            match cmd {
                TuicCommand::Packet {
                    assoc_id,
                    len,
                    addr,
                } => Ok((assoc_id, pkt.slice(cmd_len..), Address::from(addr))),
                _ => Err(Error::new(ErrorKind::InvalidData, "invalid command")),
            }
        }

        match parse_header(pkt).await {
            Ok((assoc_id, pkt, addr)) => self.handle_packet_from(assoc_id, pkt, addr).await,
            Err(err) => log::error!("[relay] [connection] {err}"),
        }
    }

    async fn process_incoming_uni_stream(self, recv: RecvStream) {
        async fn parse_header(mut recv: RecvStream) -> Result<(u32, Bytes, Address)> {
            let cmd = TuicCommand::read_from(&mut recv).await.unwrap(); // TODO: handle error

            match cmd {
                TuicCommand::Packet {
                    assoc_id,
                    len,
                    addr,
                } => {
                    let mut buf = vec![0; len as usize];
                    recv.read_exact(&mut buf).await?;
                    let pkt = Bytes::from(buf);
                    Ok((assoc_id, pkt, Address::from(addr)))
                }
                _ => Err(Error::new(ErrorKind::InvalidData, "invalid command")),
            }
        }

        match parse_header(recv).await {
            Ok((assoc_id, pkt, addr)) => self.handle_packet_from(assoc_id, pkt, addr).await,
            Err(err) => log::error!("[relay] [connection] {err}"),
        }
    }
}

pub fn channel<T>() -> (Sender<T>, Receiver<T>) {
    let (tx, rx) = oneshot::channel();
    (Sender(tx), Receiver(rx))
}

pub struct Sender<T>(OneshotSender<(Connection, T, Receiver<T>)>);

impl<T> Sender<T> {
    pub fn send(self, conn: Connection, incoming: T, next_incoming_rx: Receiver<T>) {
        // safety: the receiver must not be dropped before
        let _ = self.0.send((conn, incoming, next_incoming_rx));
    }
}

pub struct Receiver<T>(OneshotReceiver<(Connection, T, Self)>);

impl<T> Receiver<T> {
    async fn next(self) -> (Connection, T, Self) {
        // safety: the current task that waiting new incoming will be cancelled if the sender's scope is dropped
        self.0.await.unwrap()
    }
}
