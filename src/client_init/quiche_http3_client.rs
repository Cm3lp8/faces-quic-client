use log::{debug, error, info, warn};
//#[macro_use]
use mio::{event::Event, Events, Poll, Token, Waker};
use quiche::h3::{self};
use ring::rand::*;
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use crate::{
    client_config::ClientConfig,
    client_manager::{BodyQueue, Http3Request, Http3Response, RequestQueue, ResponseHead},
    my_log,
};
const MAX_DATAGRAM_SIZE: usize = 1350;
const WAKER_TOKEN: Token = Token(1);
const WAKER_TOKEN_1: Token = Token(2);
pub fn run(
    client_config: Arc<ClientConfig>,
    request_queue: RequestQueue,
    response_queue: ResponseHead,
    _body_queue: BodyQueue,
    confirm_connexion: crossbeam::channel::Sender<(String, Waker)>,
) -> Result<String, ()> {
    let mut buf = [0; 65535];
    let mut out = [0; MAX_DATAGRAM_SIZE];
    let mut last_sending_time = Duration::ZERO;
    // Cache the pending bodies if conn isn't writable
    let mut pending_bodies: HashMap<
        u64,
        Vec<(Vec<u8>, crossbeam::channel::Sender<Instant>, bool)>,
    > = HashMap::new();
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
    let mut last_instant: Option<Instant> = None;

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
    config.set_max_idle_timeout(20000);
    config.set_max_recv_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_max_send_udp_payload_size(MAX_DATAGRAM_SIZE);
    config.set_initial_max_data(100_000_000);
    config.set_initial_max_stream_data_bidi_local(100_000_000);
    config.set_initial_max_stream_data_bidi_remote(100_000_000);
    config.set_initial_max_stream_data_uni(100_000_000);
    config.set_initial_max_streams_bidi(100);
    config.set_initial_max_streams_uni(100);
    config.set_disable_active_migration(true);
    config.set_cc_algorithm(quiche::CongestionControlAlgorithm::BBR2);
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
    info!(
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

    let mut conn_confirmation = false;
    let mut req_start = std::time::Instant::now();
    let mut bodies_send = 0;
    let mut pending_count = 0;
    let mut packet_send = 0;
    let mut bytes_send = 0;
    let mut bytes_out_from_conn = 0;
    let mut lost = 0;
    let mut h3_byte_written = 0;
    let mut h3_bytes_given = 0;
    let mut round = 0;
    let mut bytes_re = 0;

    'main: loop {
        poll.poll(&mut events, conn.timeout()).unwrap();
        round += 1;
        // Read incoming UDP packets from the socket and feed them to quiche,
        // until there are no more packets to read.
        //        handle_incoming_packets(&events, &mut conn, &socket, &mut buf, local_addr);

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
            let _read = match conn.recv(&mut buf[..len], recv_info) {
                Ok(v) => v,
                Err(e) => {
                    error!("recv failed: {:?}", e);
                    continue 'read;
                }
            };
        }
        if conn.is_closed() {
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
        //handle writable
        if let Some(http3_conn) = &mut http3_conn {
            // Process HTTP/3 events.
            let trace_id = conn.trace_id().to_string();
            loop {
                match http3_conn.poll(&mut conn) {
                    Ok((stream_id, quiche::h3::Event::Headers { list, more_frames })) => {
                        let _ = waker_1.wake();
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
                            bytes_re += read;
                            let _ = waker_1.wake();
                            if let Err(e) =
                                response_queue.send_response(Http3Response::new_body_data(
                                    stream_id,
                                    trace_id.clone(),
                                    &buf[..read],
                                    false,
                                ))
                            {
                                info!("Error:  failed sending data response to response queue  [{}]   [{:?}]", stream_id, e);
                            };
                        }
                    }
                    Ok((stream_id, quiche::h3::Event::Finished)) => {
                        debug!("Finished stream [{stream_id}]!");
                        debug!("response received in {:?}, closing...", req_start.elapsed());
                        if let Err(e) = response_queue.send_response(Http3Response::new_body_data(
                            stream_id,
                            trace_id.clone(),
                            &[],
                            true,
                        )) {
                            info!("Error failed  [{}]   [{:?}]", stream_id, e);
                        };
                        let _ = waker_1.wake();
                        //     conn.close(true, 0x00, b"kthxbye").unwrap();
                    }
                    Ok((_stream_id, quiche::h3::Event::Reset(e))) => {
                        warn!("request was reset by peer with {}, closing...", e);
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

        if let Some(h3_conn) = &mut http3_conn {
            for stream_id in conn.writable() {
                if handle_writable(
                    &mut buf,
                    &events,
                    local_addr,
                    h3_conn,
                    stream_id,
                    &mut conn,
                    &mut pending_bodies,
                    &socket,
                    &mut out,
                    &mut last_sending_time,
                    &waker_1,
                    &mut packet_send,
                    &mut bytes_out_from_conn,
                    &mut h3_byte_written,
                ) {
                    if let Err(_) = waker_1.wake() {
                        error!("failed wakin mio ");
                    }
                    // continue 'main;
                }
            }

            // Send HTTP requests once the QUIC connection is established, and until
            // all requests have been sent.
            let trace_id = conn.trace_id().to_string();
            if pending_bodies.is_empty() {
                if let Some((req, adjust_send_timer)) = request_queue.pop_request() {
                    my_log::debug("%%%%%% A REQ %%%");
                    my_log::debug(&req);
                    match req {
                        Http3Request::Header(header_req) => {
                            if let Ok(stream_id) = h3_conn.send_request(
                                &mut conn,
                                header_req.headers(),
                                header_req.is_end(),
                            ) {
                                req_start = std::time::Instant::now();
                                let _ = waker_1.wake();
                                my_log::debug(format!("sended succes [{:?}]", header_req));
                                if let Err(e) = header_req.send_ids(stream_id, trace_id.as_str()) {
                                    debug!(
                                        "Error : Failed to send header request [{stream_id}], {:?}",
                                        e
                                    );
                                }
                            }
                        }
                        Http3Request::Ping(stream_id) => {
                            if !conn.stream_writable(stream_id, 1).unwrap() {
                                pending_bodies.entry(stream_id).or_default().push((
                                    vec![0x00],
                                    adjust_send_timer,
                                    false,
                                ));
                                pending_count += 1;
                            //      continue 'main;
                            } else {
                                let payload = vec![0x00];
                                match h3_conn.send_body(&mut conn, stream_id, &payload, false) {
                                    Ok(v) => {
                                        bodies_send += 1;
                                        h3_byte_written += v;
                                        if v < 1 {
                                            lost += payload.len() - v;
                                            debug!("lost total [{}]", lost);
                                            pending_bodies.entry(stream_id).or_default().push((
                                                payload[v..].to_vec(),
                                                adjust_send_timer,
                                                false,
                                            ));
                                            pending_count += 1;
                                            continue 'main;
                                        }
                                        if let Ok(_) = adjust_send_timer.send(Instant::now()) {};
                                        let _ = waker_1.wake();
                                    }
                                    Err(quiche::h3::Error::StreamBlocked) => {
                                        error!("StreamBlocked !!")
                                    }
                                    Err(e) => {
                                        error!(
                                            "Error : Failed to send ping stream [{}] {:?}",
                                            stream_id, e
                                        );
                                    }
                                }
                            }
                        }
                        Http3Request::Body(mut body_req) => {
                            if !conn.stream_writable(body_req.stream_id(), 512).unwrap() {
                                pending_bodies
                                    .entry(body_req.stream_id())
                                    .or_default()
                                    .push((
                                        body_req.take_data(),
                                        adjust_send_timer,
                                        body_req.is_end(),
                                    ));
                                pending_count += 1;
                            //      continue 'main;
                            } else {
                                match h3_conn.send_body(
                                    &mut conn,
                                    body_req.stream_id(),
                                    body_req.data(),
                                    body_req.is_end(),
                                ) {
                                    Ok(v) => {
                                        bodies_send += 1;
                                        h3_byte_written += v;
                                        if v < body_req.len() {
                                            lost += body_req.len() - v;
                                            debug!("lost total [{}]", lost);
                                            pending_bodies
                                                .entry(body_req.stream_id())
                                                .or_default()
                                                .push((
                                                    body_req.data()[v..].to_vec(),
                                                    adjust_send_timer,
                                                    body_req.is_end(),
                                                ));
                                            pending_count += 1;
                                            continue 'main;
                                        }
                                        if let Ok(_) = adjust_send_timer.send(Instant::now()) {};
                                        if !body_req.is_end() {
                                            let _ = waker_1.wake();
                                        }

                                        if body_req.is_end() {
                                            debug!(
                                                " [{}]SUCCESS ! total_writtent [{}/{}] (given) Request send ! [{}] chunks send\n Pending bodies send [{}]\n pending table is is_empty[{:?}]", body_req.stream_id(),
                                                h3_byte_written, h3_bytes_given,bodies_send, pending_count, pending_bodies.is_empty()
                                            );

                                            for i in pending_bodies.iter() {
                                                debug!("[{:?}]", i)
                                            }
                                        }
                                        if let Ok(e) = adjust_send_timer.send(Instant::now()) {}
                                    }
                                    Err(quiche::h3::Error::StreamBlocked) => {
                                        error!("StreamBlocked !!")
                                    }
                                    Err(e) => {
                                        error!(
                                    "Error : Failed to send body request [{}], packet [{}] {:?}",
                                    body_req.stream_id(),
                                    body_req.packet_id(),
                                    e
                                );
                                    }
                                }
                            }
                            /*
                            if let Ok(can_write) =
                                conn.stream_writable(body_req.stream_id(), body_req.len())
                            {
                                h3_bytes_given += body_req.len();
                                if !can_write {
                                } else {
                                }
                            }*/
                        }

                        Http3Request::BodyFromFile => {}
                    }
                }
            }
        }
        for b in pending_bodies.iter() {
            if !b.1.is_empty() {
                let _ = waker_1.wake();
                break;
            }
        }
        // Generate outgoing QUIC packets and send them on the UDP socket, until
        // quiche reports that there are no more packets to be sent.
        //
        let _w = handle_outgoing_packets(
            &mut poll,
            &mut events,
            &mut conn,
            &socket,
            &mut out,
            &waker_1,
            &mut last_sending_time,
            &mut bytes_send,
            &mut bytes_out_from_conn,
            &mut packet_send,
            &mut last_instant,
        );
        if conn.is_closed() {
            warn!("connection closed, {:?}", conn.stats());
            break Ok(conn.trace_id().to_owned());
        }
    }
}

fn hex_dump(buf: &[u8]) -> String {
    let vec: Vec<String> = buf.iter().map(|b| format!("{b:02x}")).collect();
    vec.join("")
}
/*
pub fn hdrs_to_strings(hdrs: &[quiche::h3::Header]) -> Vec<(String, String)> {
    hdrs.iter()
        .map(|h| {
            let name = String::from_utf8_lossy(h.name()).to_string();
            let value = String::from_utf8_lossy(h.value()).to_string();
            (name, value)
        })
        .collect()
}
*/
fn handle_incoming_packets_purge(
    conn: &mut quiche::Connection,
    socket: &mio::net::UdpSocket,
    buf: &mut [u8],
    local_addr: SocketAddr,
) {
    'read: loop {
        // If the event loop reported no events, it means that the timeout
        // has expired, so handle it without attempting to read packets. We
        // will then proceed with the send loop.
        let (len, from) = match socket.recv_from(buf) {
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
        let _read = match conn.recv(&mut buf[..len], recv_info) {
            Ok(v) => v,
            Err(e) => {
                debug!("recv failed: {:?}", e);
                continue 'read;
            }
        };
    }
}
fn handle_incoming_packets(
    events: &Events,
    conn: &mut quiche::Connection,
    socket: &mio::net::UdpSocket,
    buf: &mut [u8],
    local_addr: SocketAddr,
) {
    let mut read_loop_count = 0;
    let mut octets_read = 0;
    'read: loop {
        // If the event loop reported no events, it means that the timeout
        // has expired, so handle it without attempting to read packets. We
        // will then proceed with the send loop.
        if events.is_empty() {
            //             debug!("timed out");
            conn.on_timeout();
            break 'read;
        }
        let (len, from) = match socket.recv_from(buf) {
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
        octets_read += len;
        let recv_info = quiche::RecvInfo {
            to: local_addr,
            from,
        };
        // Process potentially coalesced packets.
        let _read = match conn.recv(&mut buf[..len], recv_info) {
            Ok(v) => v,
            Err(e) => {
                error!("recv failed: {:?}", e);
                continue 'read;
            }
        };
    }
}
fn handle_outgoing_packets_purge(
    conn: &mut quiche::Connection,
    socket: &mio::net::UdpSocket,
    out: &mut [u8],
    last_sending_time: &mut Duration,
    packet_send: &mut i32,
    bytes_out_from_conn: &mut usize,
) -> bool {
    let mut done = false;
    let mut last_send = Instant::now();
    let pacing_interval = Duration::from_micros(144);
    while !done {
        let pacing_instant = Instant::now();
        let (write, send_info) = match conn.send(out) {
            Ok(v) => {
                *bytes_out_from_conn += v.0;
                v
            }
            Err(quiche::Error::Done) => {
                done = true;
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
                debug!("send() would block");
                done = true;
                break;
            }
            panic!("send() failed: {:?}", e);
        }

        *last_sending_time = send_info.at.elapsed();

        if *packet_send % 10000 == 0 {
            debug!(
                "Packets uploading... [{write}] [{:?}]",
                send_info.at.elapsed()
            )
        }

        *packet_send += 1;

        while last_send.elapsed()
            < (if *last_sending_time > pacing_interval {
                *last_sending_time
            } else {
                pacing_interval
            })
        {
            std::thread::yield_now();
        }
    }
    done
}
fn handle_outgoing_packets(
    poll: &mut Poll,
    events: &mut Events,
    conn: &mut quiche::Connection,
    socket: &mio::net::UdpSocket,
    out: &mut [u8],
    waker_1: &Waker,
    last_sending_time: &mut Duration,
    byte_len_since_start: &mut u64,
    bytes_out_from_conn: &mut usize,
    packet_send: &mut i32,
    last_instant: &mut Option<Instant>,
) -> Result<usize, ()> {
    let mut packet_thres = 0;
    let mut written = 0;
    let mut pacing_interval = Duration::from_micros(60);
    let mut remaining_time: Option<Duration> = None;
    loop {
        if let Some(last_instant) = last_instant {
            if Instant::now() <= *last_instant {
                std::thread::yield_now();
                continue;
            }
            remaining_time = None;
        }
        let (write, send_info) = match conn.send(out) {
            Ok(v) => {
                written += v.0;
                *bytes_out_from_conn += 1;
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

        let path_stats = conn.path_stats().next().unwrap();
        debug!(
            "rttvar [{:?}]  [{}]  cwnd [{}]",
            path_stats.rttvar, path_stats.delivery_rate, path_stats.cwnd
        );

        *last_instant = Some(send_info.at);

        match socket.send_to(&out[..write], send_info.to) {
            Ok(v) => {
                *byte_len_since_start += v as u64;
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                debug!("send() would block");
                break;
            }
            Err(e) => {
                panic!("send() failed: {:?}", e);
            }
        }

        *last_sending_time = send_info.at.elapsed();

        packet_thres += 1;
        *packet_send += 1;
    }
    Ok(written)
}

fn handle_writable(
    in_buf: &mut [u8],
    _events: &Events,
    local_addr: SocketAddr,
    h3_conn: &mut h3::Connection,
    stream_id: u64,
    conn: &mut quiche::Connection,
    pending_map: &mut HashMap<u64, Vec<(Vec<u8>, crossbeam::channel::Sender<Instant>, bool)>>,
    socket: &mio::net::UdpSocket,
    out: &mut [u8],
    last_sending_time: &mut Duration,
    _waker_1: &Waker,
    packet_send: &mut i32,
    bytes_out_from_conn: &mut usize,
    h3_bytes_written: &mut usize,
) -> bool {
    let mut will_break_main_loop = false;
    //'purge: loop {
    let mut new_position_index: Option<usize> = None;
    if let Some(mut bodies_coll) = pending_map.remove(&stream_id) {
        //   handle_incoming_packets_purge(conn, socket, in_buf, local_addr);
        let len = bodies_coll.len();
        let mut can_write = true;
        for (i, (body, send_confirmation_to_reader, is_end)) in bodies_coll.iter_mut().enumerate() {
            if body.is_empty() {
                continue;
            }

            if let Ok(write) = conn.stream_writable(stream_id, 512) {
                can_write = write;
            }

            if can_write {
                will_break_main_loop = true;
                match h3_conn.send_body(conn, stream_id, &body, *is_end) {
                    Ok(v) => {
                        *h3_bytes_written += v;
                        if *is_end {
                            debug!("conn stats, {:?}", conn.stats());
                        }
                        debug!(
                            " sending data =[{}/{}] (len_send/total_len)  in store [{}]",
                            v,
                            body.len(),
                            len
                        );
                        if v == body.len() {
                            *body = vec![];
                        }
                        if v < body.len() {
                            new_position_index = Some(i);
                            body.drain(..v);
                        }
                    }
                    Err(h3::Error::StreamBlocked) => {
                        debug!("stream blocked");
                        new_position_index = Some(i);
                        break;
                    }
                    Err(e) => {
                        error!(
                            "[{:?}] index {i} coll [{}] packetlen [{}]",
                            e,
                            body.len(),
                            bodies_coll.len()
                        );

                        new_position_index = Some(i);
                        break;
                    }
                }
            } else {
                debug!("wait for write capaticit");
                new_position_index = Some(i);
                break;
            }
        }

        //insert one more time the pending bodies collection in the table, minus ones that have
        //been succesfully send.
        if let Some(new_start_index) = new_position_index {
            pending_map.insert(stream_id, bodies_coll[new_start_index..].to_vec());
        }
    }
    /*
    if handle_outgoing_packets_purge(
        conn,
        socket,
        out,
        last_sending_time,
        packet_send,
        bytes_out_from_conn,
    ) {
        break 'purge;
    }
    */
    // }
    will_break_main_loop
}
fn measure_output_bandwitdth(bytes_written: u64, time_since_start: Instant) -> f64 {
    let duration = time_since_start.elapsed().as_secs_f64() * 1_000_000.0;

    (bytes_written as f64 * 8.0) / duration
}
