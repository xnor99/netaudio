use std::{
    net::{ToSocketAddrs, UdpSocket},
    sync::mpsc,
};

use jack::{AudioOut, Client, Control, RingBuffer, contrib::ClosureProcessHandler};

use crate::{PACKET_SIZE, RING_BUFFER_SIZE};

// Splits interleaved stereo buffer into separate left/right iterators
fn deinterleave<T: Copy>(a: &[T]) -> Option<(impl Iterator<Item = T>, impl Iterator<Item = T>)> {
    // Ensure even number of samples
    (a.len() % 2 == 0).then(|| {
        (
            a.iter().step_by(2).copied(),         // Left channel (even indices)
            a.iter().skip(1).step_by(2).copied(), // Right channel (odd indices)
        )
    })
}

// Messages for cross-thread communication
enum Message {
    InvalidBufferLengths,
    Underrun { expected: usize, available: usize },
}

// Receiver main function
pub fn start<T: ToSocketAddrs>(client: Client, bind: T) -> Result<!, &'static str> {
    // Register JACK output ports for left and right channels
    let mut out_port_l = client
        .register_port("out_l", AudioOut::default())
        .map_err(|_| "unable to register port")?;
    let mut out_port_r = client
        .register_port("out_r", AudioOut::default())
        .map_err(|_| "unable to register port")?;

    // Bind UDP socket for receiving audio data
    let socket = UdpSocket::bind(bind).map_err(|_| "unable to bind to address")?;

    // Channel for sending warnings from audio thread to main thread
    let (sender, receiver) = mpsc::channel();

    // Create ring buffer for inter-thread communication
    let (mut ring_buffer_reader, mut ring_buffer_writer) = RingBuffer::new(RING_BUFFER_SIZE)
        .map_err(|_| "unable to create ring buffer")?
        .into_reader_writer();
    // Buffer for deinterleaving
    let mut deinterleave_channels_buffer = [0.0; RING_BUFFER_SIZE * 2];

    let _async_client = client
        .activate_async(
            (),
            ClosureProcessHandler::new(move |_, ps| {
                // Get audio buffers from JACK
                let data_to_receive_l = out_port_l.as_mut_slice(ps);
                let data_to_receive_r = out_port_r.as_mut_slice(ps);
                let amount_to_receive = data_to_receive_l.len() + data_to_receive_r.len();

                // Validate buffer sizes
                if amount_to_receive > deinterleave_channels_buffer.len()
                    || data_to_receive_l.len() != data_to_receive_r.len()
                {
                    let _ = sender.send(Message::InvalidBufferLengths);
                    return Control::Quit;
                }

                // Check for underrun (not enough data)
                let rb_space = ring_buffer_reader.space();
                if rb_space < amount_to_receive * size_of::<f32>() {
                    // Fill with silence on underrun
                    data_to_receive_l.fill(0.0);
                    data_to_receive_r.fill(0.0);
                    let _ = sender.send(Message::Underrun {
                        expected: amount_to_receive * size_of::<f32>(),
                        available: rb_space,
                    });
                } else {
                    // Read from ring buffer and deinterleave
                    ring_buffer_reader.read_buffer(bytemuck::cast_slice_mut(
                        &mut deinterleave_channels_buffer[0..amount_to_receive],
                    ));
                    // The buffer size is already multiplied by 2, so unwrapping is safe
                    let (l, r) = deinterleave(&deinterleave_channels_buffer).unwrap();
                    data_to_receive_l
                        .iter_mut()
                        .zip(l)
                        .for_each(|(buffer_val, data)| *buffer_val = data);
                    data_to_receive_r
                        .iter_mut()
                        .zip(r)
                        .for_each(|(buffer_val, data)| *buffer_val = data);
                }

                Control::Continue
            }),
        )
        .map_err(|_| "unable to activate client")?;

    // Main network receive loop
    let mut buffer = [0; PACKET_SIZE];
    loop {
        // Handle messages from audio thread
        receiver.try_iter().for_each(|message| match message {
            Message::InvalidBufferLengths => eprintln!("[WARNING] invalid buffer lengths"),
            Message::Underrun {
                expected,
                available,
            } => eprintln!(
                "[WARNING] underrun, expected to read {} bytes, {} available",
                expected, available
            ),
        });

        // Receive UDP packet
        let received = socket
            .recv_from(&mut buffer)
            .map_err(|_| "unable to receive data")?
            .0;
        if received == buffer.len() {
            // Write valid packets to ring buffer
            let rb_space = ring_buffer_writer.space();
            if rb_space >= buffer.len() {
                ring_buffer_writer.write_buffer(&buffer);
            } else {
                eprintln!(
                    "[WARNING] overrun, expected to write {} bytes, {} available",
                    buffer.len(),
                    rb_space
                );
            }
        } else {
            eprintln!(
                "[WARNING] invalid packet size, expected {}, got {}, dropping",
                PACKET_SIZE, received
            );
        }
    }
}
