/*
 * kernel.rs
 *
 * Copyright (C) 2022 by RStudio, PBC
 *
 */

use crate::connection_file::ConnectionFile;
use crate::error::Error;
use crate::language::shell_handler::ShellHandler;
use crate::session::Session;
use crate::socket::control::Control;
use crate::socket::heartbeat::Heartbeat;
use crate::socket::iopub::IOPub;
use crate::socket::iopub::IOPubMessage;
use crate::socket::shell::Shell;
use crate::socket::socket::Socket;
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread;

/// A Kernel represents a unique Jupyter kernel session and is the host for all
/// execution and messaging threads.
pub struct Kernel {
    /// The connection metadata.
    connection: ConnectionFile,

    /// The unique session information for this kernel session.
    session: Session,
}

impl Kernel {
    /// Create a new Kernel, given a connection file from a front end.
    pub fn new(file: ConnectionFile) -> Result<Kernel, Error> {
        let key = file.key.clone();

        Ok(Self {
            connection: file,
            session: Session::create(key)?,
        })
    }

    /// Connects the Kernel to the front end
    pub fn connect(
        &self,
        shell_handler: Arc<Mutex<dyn ShellHandler>>,
        iopub_sender: Sender<IOPubMessage>,
        iopub_receiver: Receiver<IOPubMessage>,
    ) -> Result<(), Error> {
        let ctx = zmq::Context::new();

        // Create the Shell ROUTER/DEALER socket and start a thread to listen
        // for client messages.
        let shell_socket = Socket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("Shell"),
            zmq::ROUTER,
            self.connection.endpoint(self.connection.shell_port),
        )?;
        thread::spawn(move || Self::shell_thread(shell_socket, iopub_sender, shell_handler));

        // Create the IOPub PUB/SUB socket and start a thread to broadcast to
        // the client. IOPub only broadcasts messages, so it listens to other
        // threads on a Receiver<Message> instead of to the client.
        let iopub_socket = Socket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("IOPub"),
            zmq::PUB,
            self.connection.endpoint(self.connection.iopub_port),
        )?;
        thread::spawn(move || Self::iopub_thread(iopub_socket, iopub_receiver));

        // Create the heartbeat socket and start a thread to listen for
        // heartbeat messages.
        let heartbeat_socket = Socket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("Heartbeat"),
            zmq::REP,
            self.connection.endpoint(self.connection.hb_port),
        )?;
        thread::spawn(move || Self::heartbeat_thread(heartbeat_socket));

        // Create the Control ROUTER/DEALER socket
        let control_socket = Socket::new(
            self.session.clone(),
            ctx.clone(),
            String::from("Control"),
            zmq::ROUTER,
            self.connection.endpoint(self.connection.control_port),
        )?;

        // TODO: thread/join thread?
        Self::control_thread(control_socket);
        Ok(())
    }

    /// Starts the control thread
    fn control_thread(socket: Socket) {
        let control = Control::new(socket);
        control.listen();
    }

    /// Starts the shell thread.
    fn shell_thread(
        socket: Socket,
        iopub_sender: Sender<IOPubMessage>,
        shell_handler: Arc<Mutex<dyn ShellHandler>>,
    ) -> Result<(), Error> {
        let mut shell = Shell::new(socket, iopub_sender.clone(), shell_handler);
        shell.listen();
        Ok(())
    }

    /// Starts the IOPub thread.
    fn iopub_thread(socket: Socket, receiver: Receiver<IOPubMessage>) -> Result<(), Error> {
        let mut iopub = IOPub::new(socket, receiver);
        iopub.listen();
        Ok(())
    }

    /// Starts the heartbeat thread.
    fn heartbeat_thread(socket: Socket) -> Result<(), Error> {
        let mut heartbeat = Heartbeat::new(socket);
        heartbeat.listen();
        Ok(())
    }
}