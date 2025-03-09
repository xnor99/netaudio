#![feature(array_chunks, never_type, try_blocks)]

use std::{env, net::SocketAddr, process::ExitCode};

use jack::{Client, ClientOptions};

// Constants defining buffer sizes for audio processing
const RING_BUFFER_SIZE: usize = 16384;
const PACKET_SIZE: usize = 480;

// Structure to hold command-line arguments
struct Args {
    bind_addr: SocketAddr,
    send_addr: Option<SocketAddr>, // Optional destination address for sender mode
}

// Parses command-line arguments into program name and optional Args
fn parse_args() -> (String, Option<Args>) {
    let mut args = env::args();
    (
        // First argument is the program name
        args.next().unwrap_or_default(),
        try {
            let bind_addr = args.next()?; // Get bind address
            let send_addr = args.next(); // Get optional send address
            Args {
                bind_addr: bind_addr.parse().ok()?,
                send_addr: send_addr.and_then(|addr| addr.parse().ok()),
            }
        },
    )
}

mod receiver;
mod sender;

fn main() -> ExitCode {
    let (program_name, args) = parse_args();
    let Some(args) = args else {
        eprintln!("USAGE: {} <bind_addr> [<send_addr>]", program_name);
        return ExitCode::FAILURE;
    };

    // Initialize JACK client with name "netaudio"
    let Ok((client, _)) = Client::new("netaudio", ClientOptions::default()) else {
        eprintln!("unable to start JACK client");
        return ExitCode::FAILURE;
    };

    eprintln!("JACK system sample rate: {} Hz", client.sample_rate());

    // Start either sender or receiver based on arguments
    let Err(error) = match args.send_addr {
        Some(send_addr) => sender::start(client, args.bind_addr, send_addr),
        None => receiver::start(client, args.bind_addr),
    };

    eprintln!("[ERROR] {}", error);
    ExitCode::FAILURE
}
