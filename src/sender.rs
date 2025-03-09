use std::{
    net::{ToSocketAddrs, UdpSocket},
    sync::mpsc::{self, RecvError},
};

use jack::{AudioIn, Client, Control, RingBuffer, contrib::ClosureProcessHandler};

use crate::{PACKET_SIZE, RING_BUFFER_SIZE};

// Combines left/right channels into interleaved iterator
fn interleave<T: Copy>(a: &[T], b: &[T]) -> Option<impl Iterator<Item = T>> {
    // Ensure equal channel lengths and interleave samples
    (a.len() == b.len()).then(|| a.iter().zip(b).flat_map(|(&l, &r)| [l, r]))
}

// Messages for cross-thread communication
enum Message {
    Ready,
    InvalidBufferLengths,
    Overrun { expected: usize, available: usize },
}

// Sender main function
pub fn start<T: ToSocketAddrs>(client: Client, bind: T, send: T) -> Result<!, &'static str> {
    // Register JACK input ports for left and right channels
    let in_port_l = client
        .register_port("in_l", AudioIn::default())
        .map_err(|_| "unable to register port")?;
    let in_port_r = client
        .register_port("in_r", AudioIn::default())
        .map_err(|_| "unable to register port")?;

    // Configure UDP socket for sending
    let socket = UdpSocket::bind(bind).map_err(|_| "unable to bind to address")?;
    socket.connect(send).map_err(|_| "unable to connect")?;

    // Channel for audio thread communication
    let (sender, receiver) = mpsc::channel();

    // Create ring buffer and interleaving buffer
    let (mut ring_buffer_reader, mut ring_buffer_writer) = RingBuffer::new(RING_BUFFER_SIZE)
        .map_err(|_| "unable to create ring buffer")?
        .into_reader_writer();
    let mut interleave_channels_buffer = [0.0; RING_BUFFER_SIZE * 2];

    let _async_client = client
        .activate_async(
            (),
            ClosureProcessHandler::new(move |_, ps| {
                // Get input audio buffers
                let data_to_send_l = in_port_l.as_slice(ps);
                let data_to_send_r = in_port_r.as_slice(ps);
                let amount_to_send = data_to_send_l.len() + data_to_send_r.len();

                // Validate buffer sizes
                if amount_to_send > interleave_channels_buffer.len()
                    || data_to_send_l.len() != data_to_send_r.len()
                {
                    let _ = sender.send(Message::InvalidBufferLengths);
                    return Control::Quit;
                }

                // Check ring buffer space
                let rb_space = ring_buffer_writer.space();
                if rb_space < amount_to_send * size_of::<f32>() {
                    let _ = sender.send(Message::Overrun {
                        expected: amount_to_send * size_of::<f32>(),
                        available: rb_space,
                    });
                } else {
                    // Interleave and write to ring buffer
                    let mut written = 0;
                    interleave_channels_buffer
                        .iter_mut()
                        // Already checked buffer sizes, so unwrapping is safe
                        .zip(interleave(data_to_send_l, data_to_send_r).unwrap())
                        .for_each(|(buffer_val, data)| {
                            *buffer_val = data;
                            written += 1;
                        });

                    ring_buffer_writer.write_buffer(bytemuck::cast_slice(
                        &interleave_channels_buffer[0..written],
                    ));
                }

                let _ = sender.send(Message::Ready);
                Control::Continue
            }),
        )
        .map_err(|_| "unable to activate client")?;

    // Main network send loop
    let mut buffer = [0; PACKET_SIZE];
    loop {
        // Wait for audio thread signal
        match receiver.recv() {
            Ok(Message::InvalidBufferLengths) => eprintln!("[ERROR] invalid buffer lengths"),
            Ok(Message::Overrun {
                expected,
                available,
            }) => eprintln!(
                "[WARNING] overrun, expected to write {} bytes, {} available",
                expected, available
            ),
            // Send when data is available
            Ok(Message::Ready) | Err(RecvError) => {
                while ring_buffer_reader.space() >= buffer.len() {
                    let data_to_send = ring_buffer_reader.read_slice(&mut buffer);
                    socket
                        .send(data_to_send)
                        .map_err(|_| "unable to send data")?;
                }
            }
        }
    }
}
