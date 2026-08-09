use super::*;

#[allow(clippy::too_many_arguments)]
pub(super) fn accept_loop<H: Handler + 'static>(
    listener: UnixListener,
    handler: Arc<H>,
    subs: Arc<Mutex<HashMap<SubId, SubscriptionLane>>>,
    journal_subs: Arc<Mutex<HashMap<SubId, SubscriptionLane>>>,
    streams: Arc<Mutex<HashMap<u64, StreamLane>>>,
    next_sub: Arc<AtomicU64>,
    next_lease: Arc<AtomicU64>,
    next_conn: Arc<AtomicU64>,
    active_connections: Arc<AtomicU32>,
) {
    for incoming in listener.incoming() {
        let Ok(stream) = incoming else {
            continue;
        };
        let Some(permit) = ConnectionPermit::acquire(&active_connections) else {
            let _ = stream.shutdown(Shutdown::Both);
            continue;
        };
        if stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT)).is_err()
            || stream.set_write_timeout(Some(WRITE_TIMEOUT)).is_err()
        {
            continue;
        }
        let h = Arc::clone(&handler);
        let s = Arc::clone(&subs);
        let js = Arc::clone(&journal_subs);
        let st = Arc::clone(&streams);
        let n = Arc::clone(&next_sub);
        let l = Arc::clone(&next_lease);
        let conn_id = next_conn.fetch_add(1, Ordering::Relaxed);
        let _ = thread::Builder::new()
            .name("aegis-ipc-conn".into())
            .spawn(move || {
                let _permit = permit;
                serve_connection(stream, h, s, js, st, n, l, conn_id);
            });
    }
}

/// Run one connection: a writer thread owns the write half; the current
/// thread drives the read half and pushes responses/events through the
/// writer's inbox. On read close the reader removes both its coarse and
/// journal subscription entries so the writer sees its last sender
/// disappear and exits promptly. Any stream the connection owned is
/// unregistered and the handler is notified once for the connection.
#[allow(clippy::too_many_arguments)]
pub(super) fn serve_connection<H: Handler + 'static>(
    stream: UnixStream,
    handler: Arc<H>,
    subs: Arc<Mutex<HashMap<SubId, SubscriptionLane>>>,
    journal_subs: Arc<Mutex<HashMap<SubId, SubscriptionLane>>>,
    streams: Arc<Mutex<HashMap<u64, StreamLane>>>,
    next_sub: Arc<AtomicU64>,
    next_lease: Arc<AtomicU64>,
    conn_id: u64,
) {
    let mut read_half = match stream.try_clone() {
        Ok(s) => s,
        Err(_) => return,
    };
    let shutdown = match stream.try_clone() {
        Ok(stream) => Arc::new(stream),
        Err(_) => return,
    };
    let (tx, rx) = mpsc::sync_channel::<Outbound>(OUTBOUND_QUEUE_DEPTH);
    let writer_handler = Arc::clone(&handler);
    let writer_streams = Arc::clone(&streams);

    let writer = thread::Builder::new()
        .name("aegis-ipc-writer".into())
        .spawn(move || {
            let mut w = stream;
            while let Ok(out) = rx.recv() {
                let res = match out {
                    Outbound::Response(mut response) => {
                        let result = write_msg(&mut w, &response);
                        response.zeroize_sensitive();
                        result
                    }
                    Outbound::Event(e) => write_msg(&mut w, &e),
                    Outbound::StreamStarted { response, table } => {
                        write_stream_started(&mut w, &response, &table)
                    }
                    Outbound::CaptureOutput {
                        payload,
                        lease_deadline,
                        scope,
                    } => write_output_capture(
                        &mut w,
                        payload,
                        lease_deadline,
                        &*writer_handler,
                        &scope,
                    ),
                    Outbound::CaptureInteractionDomain {
                        payload,
                        lease_deadline,
                        scope,
                        via_grant,
                    } => write_interaction_domain_capture(
                        &mut w,
                        payload,
                        lease_deadline,
                        &*writer_handler,
                        &scope,
                        via_grant,
                    ),
                    Outbound::StreamFrame {
                        payload,
                        lease_deadline,
                        scope,
                        target,
                        queued,
                    } => {
                        let res = write_stream_frame(
                            &mut w,
                            payload,
                            &*writer_handler,
                            &scope,
                            target,
                            &lease_deadline,
                            &writer_streams,
                        );
                        queued.fetch_sub(1, Ordering::AcqRel);
                        res
                    }
                };
                if res.is_err() {
                    break;
                }
            }
        });

    let (sub_id, journal_sub_id, idle_inhibited) = drive_read_loop(
        &mut read_half,
        &tx,
        &*handler,
        &subs,
        &journal_subs,
        &streams,
        &next_sub,
        &next_lease,
        &shutdown,
        conn_id,
    );
    if let Some(id) = sub_id {
        subs.lock().unwrap().remove(&id);
    }
    if let Some(id) = journal_sub_id {
        journal_subs.lock().unwrap().remove(&id);
    }
    handler.connection_disconnected(conn_id);
    // A disconnect releases the connection's idle inhibitor fail-closed,
    // exactly like its streams (ADR-0075).
    if idle_inhibited {
        handler.idle_inhibit_disconnected(conn_id);
    }
    let owned_streams: Vec<u64> = {
        let mut streams = streams.lock().unwrap();
        let owned: Vec<u64> = streams
            .iter()
            .filter(|(_, lane)| lane.conn_id == conn_id)
            .map(|(id, _)| *id)
            .collect();
        for id in &owned {
            streams.remove(id);
        }
        owned
    };
    if !owned_streams.is_empty() {
        handler.streams_disconnected(conn_id);
    }
    drop(tx);
    if let Ok(handle) = writer {
        let _ = handle.join();
    }
}
