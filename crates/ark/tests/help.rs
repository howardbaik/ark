//
// help.rs
//
// Copyright (C) 2023 Posit Software, PBC. All rights reserved.
//
//

use core::panic;

use amalthea::comm::comm_channel::CommChannelMsg;
use amalthea::socket::comm::CommInitiator;
use amalthea::socket::comm::CommSocket;
use ark::help::message::HelpMessage;
use ark::help::message::HelpReply;
use ark::help::message::HelpRequest;
use ark::help::message::ShowTopicRequest;
use ark::help::r_help::RHelp;
use ark::modules;
use ark::r_task;
use harp::exec::RFunction;
use harp::test::start_r;

/**
 * Basic test for the R help comm; requests help for a topic and ensures that we
 * get a reply.
 */
#[test]
fn test_help_comm() {
    start_r();

    // Initialize the modules so that the help system has access to its .ps.help
    // methods.
    unsafe {
        modules::initialize().unwrap();
    }

    // Create the comm socket for the Help comm
    let comm = CommSocket::new(
        CommInitiator::FrontEnd,
        String::from("test-help-comm-id"),
        String::from("positron.help"),
    );

    let incoming_tx = comm.incoming_tx.clone();
    let outgoing_rx = comm.outgoing_rx.clone();

    // Start the help comm. It's important to save the help request sender so
    // that the help comm doesn't exit before we're done with it; allowing the
    // sender to be dropped signals the help comm to exit.
    let (help_request_tx, help_reply_rx) = RHelp::start(comm).unwrap();

    // Send a request for the help topic 'library'
    let request = HelpMessage::ShowHelpTopicRequest(ShowTopicRequest {
        topic: String::from("library"),
    });
    let data = serde_json::to_value(request).unwrap();
    let request_id = String::from("help-test-id-1");
    incoming_tx
        .send(CommChannelMsg::Rpc(request_id.clone(), data))
        .unwrap();

    // Wait for the response (up to 1 second; this should be fast!)
    let duration = std::time::Duration::from_secs(1);
    let response = outgoing_rx.recv_timeout(duration).unwrap();
    match response {
        CommChannelMsg::Rpc(id, val) => {
            let response = serde_json::from_value::<HelpMessage>(val).unwrap();
            match response {
                HelpMessage::ShowHelpTopicReply(_reply) => {
                    // Ensure we got a reply with an ID that matches the request
                    assert_eq!(id, request_id);
                },
                _ => {
                    panic!("Unexpected message from help comm: {:?}", response);
                },
            }
        },
        _ => {
            panic!("Unexpected response from help comm: {:?}", response);
        },
    }

    // Send a request to show a help URL. This URL isn't in help format, so we
    // don't expect it to be handled.
    let url = String::from("https://www.example.com");
    let request = HelpRequest::ShowHelpUrlRequest(url);
    help_request_tx.send(request).unwrap();
    let response = help_reply_rx.recv_timeout(duration).unwrap();
    let handled = match response {
        HelpReply::ShowHelpUrlReply(handled) => handled,
    };
    assert_eq!(handled, false);

    // Figure out which port the R help server is running on (or would run on)
    let r_help_port =
        r_task(|| unsafe { RFunction::new("tools", "httpdPort").call()?.to::<u16>() }).unwrap();

    // Send a request to show a help URL with a valid help URL. This one should
    // be handled.
    let url = format!(
        "http://127.0.0.1:{}/library/base/html/plot.html",
        r_help_port
    );
    let request = HelpRequest::ShowHelpUrlRequest(url);
    help_request_tx.send(request).unwrap();
    let response = help_reply_rx.recv_timeout(duration).unwrap();
    let handled = match response {
        HelpReply::ShowHelpUrlReply(handled) => handled,
    };
    assert_eq!(handled, true);
}