//
// dap_server.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use std::io::{BufReader, BufWriter, Read, Write};
use std::sync::{Arc, Mutex};
use stdext::spawn;

use crossbeam::channel::Sender;
use dap::events::*;
use dap::prelude::*;
use dap::requests::*;
use dap::responses::*;
use dap::types::*;
use harp::session::FrameInfo;
use stdext::result::ResultOrLog;

use super::dap::DapState;
use crate::dap::dap_event_loop::DapEventLoop;

const THREAD_ID: i64 = -1;

pub fn start_dap(tcp_address: String, state: Arc<Mutex<DapState>>, conn_init_tx: Sender<bool>) {
    log::trace!("DAP: Thread starting at address {}.", tcp_address);

    // Start with a blocking connection to simplify things at connection time
    let listener = std::net::TcpListener::bind(tcp_address).unwrap();

    conn_init_tx
        .send(true)
        .or_log_error("DAP: Can't send init notification");

    loop {
        log::trace!("DAP: Waiting for client");

        let tcp_stream = match listener.accept() {
            Ok((stream, addr)) => {
                log::info!("DAP: Connected to client {addr:?}");
                stream
            },
            Err(e) => {
                log::error!("DAP: Can't get client: {e:?}");
                continue;
            },
        };

        let mut el = DapEventLoop::new(tcp_stream);
        let (dap_incoming_reader, dap_outgoing_writer) = el.dap_streams();

        spawn!("ark-dap-event-loop", move || {
            if let Err(err) = el.event_loop() {
                log::error!("DAP event loop thread terminated abruptly: {err}");
            }
        });

        let mut server = DapServer::new(dap_incoming_reader, dap_outgoing_writer, state.clone());

        loop {
            // If disconnected, break and accept a new connection to create a new server
            if !server.serve() {
                log::trace!("DAP: Disconnected from client");
                state.lock().unwrap().debugging = false;
                break;
            }
        }

        // The end of this scope drops the sending side of
        // `bridge_outgoing_reader` which shuts downs the event loop thread
    }
}

pub struct DapServer<R: Read, W: Write> {
    server: Server<R, W>,
    state: Arc<Mutex<DapState>>,
}

impl<R: Read, W: Write> DapServer<R, W> {
    pub fn new(reader: BufReader<R>, writer: BufWriter<W>, state: Arc<Mutex<DapState>>) -> Self {
        Self {
            server: Server::new(reader, writer),
            state,
        }
    }

    pub fn serve(&mut self) -> bool {
        log::trace!("DAP: Polling");
        let req = match self.server.poll_request().unwrap() {
            Some(req) => req,
            None => {
                // TODO: Quit debugger if not busy
                return false;
            },
        };
        log::trace!("DAP: Got request: {:?}", req);

        let cmd = req.command.clone();

        match cmd {
            Command::Initialize(args) => {
                self.handle_initialize(req, args);
            },
            Command::Attach(args) => {
                self.handle_attach(req, args);
            },
            Command::Threads => {
                self.handle_threads(req);
            },
            Command::SetExceptionBreakpoints(args) => {
                self.handle_set_exception_breakpoints(req, args);
            },
            Command::StackTrace(args) => {
                self.handle_stacktrace(req, args);
            },
            _ => {
                log::warn!("DAP: Unknown request");
                let rsp = req.error("Ark DAP: Unknown request");
                self.server.respond(rsp).unwrap();
            },
        }

        true
    }

    fn handle_initialize(&mut self, req: Request, _args: InitializeArguments) {
        let rsp = req.success(ResponseBody::Initialize(types::Capabilities {
            ..Default::default()
        }));
        self.server.respond(rsp).unwrap();

        self.server.send_event(Event::Initialized).unwrap();
    }

    fn handle_attach(&mut self, req: Request, _args: AttachRequestArguments) {
        let rsp = req.success(ResponseBody::Attach);
        self.server.respond(rsp).unwrap();

        self.server
            .send_event(Event::Stopped(StoppedEventBody {
                reason: StoppedEventReason::Step,
                description: Some(String::from("Execution paused")),
                thread_id: Some(THREAD_ID),
                preserve_focus_hint: Some(false),
                text: None,
                all_threads_stopped: None,
                hit_breakpoint_ids: None,
            }))
            .unwrap();
    }

    // All servers must respond to `Threads` requests, possibly with
    // a dummy thread as is the case here
    fn handle_threads(&mut self, req: Request) {
        let rsp = req.success(ResponseBody::Threads(ThreadsResponse {
            threads: vec![Thread {
                id: THREAD_ID,
                name: String::from("Main thread"),
            }],
        }));
        self.server.respond(rsp).unwrap();
    }

    fn handle_set_exception_breakpoints(
        &mut self,
        req: Request,
        _args: SetExceptionBreakpointsArguments,
    ) {
        let rsp = req.success(ResponseBody::SetExceptionBreakpoints(
            SetExceptionBreakpointsResponse {
                breakpoints: None, // TODO
            },
        ));
        self.server.respond(rsp).unwrap();
    }

    fn handle_stacktrace(&mut self, req: Request, _args: StackTraceArguments) {
        let stack = { self.state.lock().unwrap().stack.clone() };

        let stack = match stack {
            Some(s) if s.len() > 0 => s.into_iter().map(into_dap_frame).collect(),
            _ => vec![],
        };

        let rsp = req.success(ResponseBody::StackTrace(StackTraceResponse {
            stack_frames: stack,
            total_frames: Some(1),
        }));

        self.server.respond(rsp).unwrap();
    }
}

fn into_dap_frame(frame: FrameInfo) -> StackFrame {
    let name = frame.name.clone();
    let path = frame.file.clone();
    let line = frame.line;
    let column = frame.column;

    let src = Source {
        name: None,
        path: Some(path),
        source_reference: None,
        presentation_hint: None,
        origin: None,
        sources: None,
        adapter_data: None,
        checksums: None,
    };

    StackFrame {
        id: THREAD_ID,
        name,
        source: Some(src),
        line,
        column,
        end_line: None,
        end_column: None,
        can_restart: None,
        instruction_pointer_reference: None,
        module_id: None,
        presentation_hint: None,
    }
}
