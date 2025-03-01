use crossbeam::thread;
use log::{debug, error, info, warn};
//#[macro_use]
use mio::{Interest, Token, Waker};
use quiche::h3::NameValue;
use ring::rand::*;
use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    client_config::ClientConfig,
    client_manager::{BodyQueue, Http3Request, Http3Response, RequestQueue, ResponseHead},
};
const MAX_DATAGRAM_SIZE: usize = 1350;
const WAKER_TOKEN: Token = Token(1);
const WAKER_TOKEN_1: Token = Token(2);
pub fn run(
    client_config: Arc<ClientConfig>,
    request_queue: RequestQueue,
    response_queue: ResponseHead,
    body_queue: BodyQueue,
    confirm_connexion: crossbeam::channel::Sender<(String, Waker)>,
) -> Result<String, ()> {
    let mut buf = [0; 65535];
    let mut out = [0; MAX_DATAGRAM_SIZE];
    let mut last_sending_time = Duration::ZERO;
    //    let url = url::Url::parse(&args.next().unwrap()).unwrap();
    // Setup the event loop.
    let mut poll = mio::Poll::new().unwrap();
    let mut events = mio::Events::with_capacity(8192);
    // Resolve server address.
    //    let peer_addr = url.to_socket_addrs().unwrap().next().unwrap();
    let peer_addr = client_config.peer_address().unwrap();
    // Bind to INADDR_ANY or IN6ADDR_ANY depending on the IP family of the
    // server address. This is needed on macOS and BSD variants that don't
    // support binding to IN6ADDR_ANY for both v4 and v6.
    let bind_addr = client_config.local_address().unwrap();
    // Create the UDP socket backing the QUIC connection, and register it with
    // the event loop.
    let mut socket = mio::net::UdpSocket::bind(bind_addr).unwrap();

    let mut waker = Some(Waker::new(poll.registry(), WAKER_TOKEN).unwrap());
    let waker_1 = Waker::new(poll.registry(), WAKER_TOKEN_1).unwrap();
    poll.registry()
        .register(
            &mut socket,
            mio::Token(0),
            mio::Interest::READABLE | mio::Interest::WRITABLE,
        )
        .unwrap();
    // Create the configuration for the QUIC connection.
    let mut config = quiche::Config::new(quiche::PROTOCOL_VERSION).unwrap();
    // *CAUTION*: this should not be set to `false` in production!!!
    config.verify_peer(false);
    config
        .set_application_protos(quiche::h3::APPLICATION_PROTOCOL)
        .unwrap();
    config.set_max_idle_timeout(5000);
    config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_initial_max_data(50_000_000);
    config.set_initial_max_stream_data_bidi_local(100_000_000);
    config.set_initial_max_stream_data_bidi_remote(100_000_000);
    config.set_initial_max_stream_data_uni(10_000_000);
    config.set_initial_max_streams_bidi(100);
    config.set_initial_max_streams_uni(100);
    config.set_disable_active_migration(true);
    config.set_cc_algorithm(quiche::CongestionControlAlgorithm::Reno);
    let mut http3_conn = None;
    // Generate a random source connection ID for the connection.
    let mut scid = [0; quiche::MAX_CONN_ID_LEN];
    SystemRandom::new().fill(&mut scid[..]).unwrap();
    let scid = quiche::ConnectionId::from_ref(&scid);
    // Get local address.
    let local_addr = socket.local_addr().unwrap();
    // Create a QUIC connection and initiate handshake.
    let mut conn =
        quiche::connect(Some("quichec"), &scid, local_addr, peer_addr, &mut config).unwrap();
    debug!(
        "connecting to {:} from {:} with scid {}",
        peer_addr,
        socket.local_addr().unwrap(),
        hex_dump(&scid)
    );
    let (write, send_info) = conn.send(&mut out).expect("initial send failed");
    while let Err(e) = socket.send_to(&out[..write], send_info.to) {
        if e.kind() == std::io::ErrorKind::WouldBlock {
            debug!("send() would block");
            continue;
        }
        panic!("send() failed: {:?}", e);
    }
    let h3_config = quiche::h3::Config::new().unwrap();
    // Prepare request.
    /*
        let path = b"/";
        let req = vec![
            quiche::h3::Header::new(b":method", b"GET"),
            quiche::h3::Header::new(b":scheme", b"https"),
            quiche::h3::Header::new(b":authority", b"127.0.0.1:3000"),
            quiche::h3::Header::new(b":path", path),
            quiche::h3::Header::new(b"user-agent", b"quiche"),
        ];
    */
    let mut conn_confirmation = false;
    let req_start = std::time::Instant::now();
    let mut bodies_send = 0;
    let mut packet_send = 0;
    loop {
        poll.poll(&mut events, conn.timeout()).unwrap();
        // Read incoming UDP packets from the socket and feed them to quiche,
        // until there are no more packets to read.
        'read: loop {
            // If the event loop reported no events, it means that the timeout
            // has expired, so handle it without attempting to read packets. We
            // will then proceed with the send loop.
            if events.is_empty() {
                //             debug!("timed out");
                conn.on_timeout();
                break 'read;
            }
            let (len, from) = match socket.recv_from(&mut buf) {
                Ok(v) => v,
                Err(e) => {
                    // There are no more UDP packets to read, so end the read
                    // loop.
                    if e.kind() == std::io::ErrorKind::WouldBlock {
                        debug!("recv() would block");
                        break 'read;
                    }
                    panic!("recv() failed: {:?}", e);
                }
            };
            let recv_info = quiche::RecvInfo {
                to: local_addr,
                from,
            };
            // Process potentially coalesced packets.
            let read = match conn.recv(&mut buf[..len], recv_info) {
                Ok(v) => v,
                Err(e) => {
                    debug!("recv failed: {:?}", e);
                    continue 'read;
                }
            };
        }
        if conn.is_closed() {
            debug!("connection closed, {:?}", conn.stats());
            break Ok(conn.trace_id().to_owned());
        }
        // Create a new HTTP/3 connection once the QUIC connection is established.
        if conn.is_established() && http3_conn.is_none() {
            http3_conn = Some(
                quiche::h3::Connection::with_transport(&mut conn, &h3_config)
                .expect("Unable to create HTTP/3 connection, check the server's uni stream limit and window size"),
            );

            if !conn_confirmation {
                if let Err(e) =
                    confirm_connexion.send((conn.trace_id().to_string(), waker.take().unwrap()))
                {
                    debug!(
                        "Error : failed to send connxion confirmation for [{:?}]   [{:?}]",
                        conn.trace_id(),
                        e
                    );
                }
                conn_confirmation = true;
            }
        }
        // Send HTTP requests once the QUIC connection is established, and until
        // all requests have been sent.
        if let Some(h3_conn) = &mut http3_conn {
            let trace_id = conn.trace_id().to_string();
            if let Some((req, adjust_send_timer)) = request_queue.pop_request() {
                match req {
                    Http3Request::Header(header_req) => {
                        if let Ok(stream_id) = h3_conn.send_request(
                            &mut conn,
                            header_req.headers(),
                            header_req.is_end(),
                        ) {
                            debug!("sended succes [{:?}]", header_req);
                            if let Err(e) = header_req.send_ids(stream_id, trace_id.as_str()) {
                                debug!(
                                    "Error : Failed to send header request [{stream_id}], {:?}",
                                    e
                                );
                            }
                        }
                    }
                    Http3Request::Body(body_req) => {
                        debug!("sending body succes [{:?}]", body_req.data().len());
                        if let Err(e) = h3_conn.send_body(
                            &mut conn,
                            body_req.stream_id(),
                            body_req.data(),
                            body_req.is_end(),
                        ) {
                            error!(
                                "Error : Failed to send body request [{}], packet [{}] {:?}",
                                body_req.stream_id(),
                                body_req.packet_id(),
                                e
                            );
                        } else {
                            bodies_send += 1;
                            if body_req.is_end() {
                                info!("Success ! Request send ! [{}] chunks send", bodies_send);
                            }
                        }
                    }
                    Http3Request::BodyFromFile => {}
                }
                if let Err(e) = adjust_send_timer.send(last_sending_time) {
                    error!("Failed sending adjust_send_timer [{:?}]", e);
                }
            }
        }
        if let Some(http3_conn) = &mut http3_conn {
            // Process HTTP/3 events.
            let trace_id = conn.trace_id().to_string();
            loop {
                match http3_conn.poll(&mut conn) {
                    Ok((stream_id, quiche::h3::Event::Headers { list, more_frames })) => {
                        /*
                                                debug!(
                                                    "got response headers {:?} on stream id {}",
                                                    hdrs_to_strings(&list),
                                                    stream_id
                                                );
                        */
                        if let Err(e) = response_queue.send_response(Http3Response::new_header(
                            stream_id,
                            trace_id.clone(),
                            list,
                            !more_frames,
                        )) {
                            debug!(
                                "Error failed to send response back [{}]   [{:?}]",
                                stream_id, e
                            );
                        };
                    }
                    Ok((stream_id, quiche::h3::Event::Data)) => {
                        while let Ok(read) = http3_conn.recv_body(&mut conn, stream_id, &mut buf) {
                            /*
                                                        debug!(
                                                            "got {} bytes of response data on stream {}",
                                                            read, stream_id
                                                        );
                            */
                            if let Err(e) =
                                response_queue.send_response(Http3Response::new_body_data(
                                    stream_id,
                                    trace_id.clone(),
                                    &buf[..read],
                                    false,
                                ))
                            {
                                debug!("Error:  failed sending data response to response queue  [{}]   [{:?}]", stream_id, e);
                            };
                            //   print!("{}", unsafe { std::str::from_utf8_unchecked(&buf[..read]) });
                        }
                    }
                    Ok((stream_id, quiche::h3::Event::Finished)) => {
                        debug!("response received in {:?}, closing...", req_start.elapsed());
                        if let Err(e) = response_queue.send_response(Http3Response::new_body_data(
                            stream_id,
                            trace_id.clone(),
                            &[],
                            true,
                        )) {
                            debug!("Error failed  [{}]   [{:?}]", stream_id, e);
                        };
                        //     conn.close(true, 0x00, b"kthxbye").unwrap();
                    }
                    Ok((_stream_id, quiche::h3::Event::Reset(e))) => {
                        debug!("request was reset by peer with {}, closing...", e);
                        conn.close(true, 0x00, b"kthxbye").unwrap();
                    }
                    Ok((_, quiche::h3::Event::PriorityUpdate)) => unreachable!(),
                    Ok((goaway_id, quiche::h3::Event::GoAway)) => {
                        debug!("GOAWAY id={}", goaway_id);
                    }
                    Err(quiche::h3::Error::Done) => {
                        break;
                    }
                    Err(e) => {
                        debug!("HTTP/3 processing failed: {:?}", e);
                        break;
                    }
                }
            }
        }
        // Generate outgoing QUIC packets and send them on the UDP socket, until
        // quiche reports that there are no more packets to be sent.
        let mut last_send = Instant::now();
        let mut pacing_interval = Duration::from_micros(1);
        let mut packet_thres = 0;
        let mut pacing_instant = Instant::now();
        loop {
            let (write, send_info) = match conn.send(&mut out) {
                Ok(v) => {
                    pacing_instant = send_info.at;
                    v
                }
                Err(quiche::Error::Done) => {
                    break;
                }
                Err(e) => {
                    error!("send failed: {:?}", e);
                    conn.close(false, 0x1, b"fail").ok();
                    break;
                }
            };
            if let Err(e) = socket.send_to(&out[..write], send_info.to) {
                if e.kind() == std::io::ErrorKind::WouldBlock {
                    warn!("send() would block");
                    break;
                }
                panic!("send() failed: {:?}", e);
            }

            last_sending_time = send_info.at.elapsed();

            if packet_send % 10000 == 0 {
                debug!("Packets uploading... [{write}]")
            }

            packet_thres += 1;
            packet_send += 1;
            if packet_thres >= 1000 {
                if let Err(e) = waker_1.wake() {
                    error!("wake error [{:?}]", e);
                }
                warn!("wake !!");
                break;
            }

            /*
                        while last_send.elapsed() < pacing_interval {
                            std::thread::yield_now();
                        }
            */
        }
        if let Err(e) = waker_1.wake() {
            error!("wake error [{:?}]", e);
        }
        if conn.is_closed() {
            debug!("connection closed, {:?}", conn.stats());
            break Ok(conn.trace_id().to_owned());
        }
    }
}
fn hex_dump(buf: &[u8]) -> String {
    let vec: Vec<String> = buf.iter().map(|b| format!("{b:02x}")).collect();
    vec.join("")
}
pub fn hdrs_to_strings(hdrs: &[quiche::h3::Header]) -> Vec<(String, String)> {
    hdrs.iter()
        .map(|h| {
            let name = String::from_utf8_lossy(h.name()).to_string();
            let value = String::from_utf8_lossy(h.value()).to_string();
            (name, value)
        })
        .collect()
}
