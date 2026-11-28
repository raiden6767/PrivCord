//! Bytes from the network that the TURN side and the address finder read: stream framing, and a STUN answer.
#![no_main]
use libfuzzer_sys::fuzz_target;

fuzz_target!(|data: &[u8]| {
    if let Some(head) = data.get(..4) {
        if let Some((total, padded)) = privcord_server::turn_stream::frame_lengths([head[0], head[1], head[2], head[3]]) {
            assert!(total <= padded && padded <= 8192);
        }
    }
    let _ = privcord_server::nat::parse_response(data, &[7u8; 12]);
    // an answer that does answer our request, with the rest arbitrary
    let mut answer = vec![0x01, 0x01, 0, 0, 0x21, 0x12, 0xa4, 0x42];
    answer.extend_from_slice(&[7u8; 12]);
    answer.extend_from_slice(data);
    let _ = privcord_server::nat::parse_response(&answer, &[7u8; 12]);
});
