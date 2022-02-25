/*
 * shell.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::error::Error;
use crate::language::shell_handler::ShellHandler;
use crate::socket::iopub::IOPubMessage;
use crate::socket::socket::Socket;
use crate::wire::comm_info_reply::CommInfoReply;
use crate::wire::comm_info_request::CommInfoRequest;
use crate::wire::complete_reply::CompleteReply;
use crate::wire::complete_request::CompleteRequest;
use crate::wire::execute_request::ExecuteRequest;
use crate::wire::is_complete_reply::IsCompleteReply;
use crate::wire::is_complete_request::IsCompleteRequest;
use crate::wire::jupyter_message::JupyterMessage;
use crate::wire::jupyter_message::Message;
use crate::wire::jupyter_message::ProtocolMessage;
use crate::wire::kernel_info_reply::KernelInfoReply;
use crate::wire::kernel_info_request::KernelInfoRequest;
use crate::wire::status::ExecutionState;
use crate::wire::status::KernelStatus;
use log::{debug, trace, warn};
use std::sync::mpsc::Sender;
use std::sync::{Arc, Mutex};

/// Wrapper for the Shell socket; receives requests for execution, etc. from the
/// front end and handles them or dispatches them to the execution thread.
pub struct Shell {
    /// The ZeroMQ Shell socket
    socket: Socket,

    /// Sends messages to the IOPub socket (owned by another thread)
    iopub_sender: Sender<IOPubMessage>,

    /// Language-provided shell handler object
    handler: Arc<Mutex<dyn ShellHandler>>,
}

impl Shell {
    /// Create a new Shell socket.
    ///
    /// * `socket` - The underlying ZeroMQ Shell socket
    /// * `iopub_sender` - A channel that delivers messages to the IOPub socket
    /// * `handler` - The language's shell channel handler
    pub fn new(
        socket: Socket,
        iopub_sender: Sender<IOPubMessage>,
        handler: Arc<Mutex<dyn ShellHandler>>,
    ) -> Self {
        Self {
            socket: socket,
            iopub_sender: iopub_sender,
            handler: handler,
        }
    }

    /// Main loop for the Shell thread; to be invoked by the kernel.
    pub fn listen(&mut self) {
        loop {
            trace!("Waiting for shell messages");
            // Attempt to read the next message from the ZeroMQ socket
            let message = match Message::read_from_socket(&self.socket) {
                Ok(m) => m,
                Err(err) => {
                    warn!("Could not read message from shell socket: {}", err);
                    continue;
                }
            };

            // Handle the message; any failures while handling the messages are
            // delivered to the client instead of reported up the stack, so the
            // only errors likely here are "can't deliver to client"
            if let Err(err) = self.process_message(message) {
                warn!("Could not handle shell message: {}", err);
            }
        }
    }

    /// Process a message received from the front-end, optionally dispatching
    /// messages to the IOPub or execution threads
    fn process_message(&mut self, msg: Message) -> Result<(), Error> {
        let result = match msg {
            Message::KernelInfoRequest(req) => {
                self.handle_request(req, |h, r| self.handle_info_request(h, r))
            }
            Message::IsCompleteRequest(req) => {
                self.handle_request(req, |h, r| self.handle_is_complete_request(h, r))
            }
            Message::ExecuteRequest(req) => {
                self.handle_request(req, |h, r| self.handle_execute_request(h, r))
            }
            Message::CompleteRequest(req) => {
                self.handle_request(req, |h, r| self.handle_complete_request(h, r))
            }
            Message::CommInfoRequest(req) => {
                self.handle_request(req, |h, r| self.handle_comm_info_request(h, r))
            }
            _ => Err(Error::UnsupportedMessage(msg, String::from("shell"))),
        };

        result
    }

    /// Wrapper for all request handlers; emits busy, invokes the handler, then
    /// emits idle. Most frontends expect all shell messages to be wrapped in
    /// this pair of statuses.
    fn handle_request<
        T: ProtocolMessage,
        H: Fn(&mut dyn ShellHandler, JupyterMessage<T>) -> Result<(), Error>,
    >(
        &self,
        req: JupyterMessage<T>,
        handler: H,
    ) -> Result<(), Error> {
        use std::ops::DerefMut;

        // Enter the kernel-busy state in preparation for handling the message.
        if let Err(err) = self.send_state(req.clone(), ExecutionState::Busy) {
            warn!("Failed to change kernel status to busy: {}", err)
        }

        // Lock the shell handler object on this thread
        let mut shell_handler = self.handler.lock().unwrap();

        // Handle the message!
        let result = handler(shell_handler.deref_mut(), req.clone());

        // Return to idle -- we always do this, even if the message generated an
        // error, since many front ends won't submit additional messages until
        // the kernel is marked idle.
        if let Err(err) = self.send_state(req, ExecutionState::Idle) {
            warn!("Failed to restore kernel status to idle: {}", err)
        }
        result
    }

    /// Sets the kernel state by sending a message on the IOPub channel.
    fn send_state<T: ProtocolMessage>(
        &self,
        parent: JupyterMessage<T>,
        state: ExecutionState,
    ) -> Result<(), Error> {
        let reply = KernelStatus {
            execution_state: state,
        };
        if let Err(err) = self
            .iopub_sender
            .send(IOPubMessage::Status(parent.header, reply))
        {
            return Err(Error::SendError(format!("{}", err)));
        }
        Ok(())
    }

    /// Handles an ExecuteRequest; dispatches the request to the execution
    /// thread and forwards the response
    fn handle_execute_request(
        &self,
        handler: &mut dyn ShellHandler,
        req: JupyterMessage<ExecuteRequest>,
    ) -> Result<(), Error> {
        debug!("Received execution request {:?}", req);
        match handler.handle_execute_request(&req.content) {
            Ok(reply) => req.send_reply(reply, &self.socket),
            Err(err) => req.send_reply(err, &self.socket),
        }
    }

    /// Handle a request to test code for completion.
    fn handle_is_complete_request(
        &self,
        handler: &dyn ShellHandler,
        req: JupyterMessage<IsCompleteRequest>,
    ) -> Result<(), Error> {
        debug!("Received request to test code for completeness: {:?}", req);
        match handler.handle_is_complete_request(&req.content) {
            Ok(reply) => req.send_reply(reply, &self.socket),
            Err(err) => req.send_error::<IsCompleteReply>(err, &self.socket),
        }
    }

    /// Handle a request for kernel information.
    fn handle_info_request(
        &self,
        handler: &dyn ShellHandler,
        req: JupyterMessage<KernelInfoRequest>,
    ) -> Result<(), Error> {
        debug!("Received shell information request: {:?}", req);
        match handler.handle_info_request(&req.content) {
            Ok(reply) => req.send_reply(reply, &self.socket),
            Err(err) => req.send_error::<KernelInfoReply>(err, &self.socket),
        }
    }

    /// Handle a request for code completion.
    fn handle_complete_request(
        &self,
        handler: &dyn ShellHandler,
        req: JupyterMessage<CompleteRequest>,
    ) -> Result<(), Error> {
        debug!("Received request to complete code: {:?}", req);
        match handler.handle_complete_request(&req.content) {
            Ok(reply) => req.send_reply(reply, &self.socket),
            Err(err) => req.send_error::<CompleteReply>(err, &self.socket),
        }
    }

    /// Handle a request for open comms
    fn handle_comm_info_request(
        &self,
        handler: &dyn ShellHandler,
        req: JupyterMessage<CommInfoRequest>,
    ) -> Result<(), Error> {
        debug!("Received request for open comms: {:?}", req);
        match handler.handle_comm_info_request(&req.content) {
            Ok(reply) => req.send_reply(reply, &self.socket),
            Err(err) => req.send_error::<CommInfoReply>(err, &self.socket),
        }
    }
}