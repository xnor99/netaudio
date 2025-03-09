# netaudio - Real-time audio streaming over UDP

Low-latency audio streaming between JACK-enabled systems using UDP. Acts as either a sender (streams audio) or receiver (plays streamed audio).

## Configuration
Currently not very configurable. Tune `PACKET_SIZE` and `RING_BUFFER_SIZE` constants in `src/main.rs` and recompile.

Requires Rust nightly.
