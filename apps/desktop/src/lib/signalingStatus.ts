/** How the relays as a whole are doing. The Rust engine's signaling layer decides it; this is only its name here. */
export type SignalingStatus = 'idle' | 'connecting' | 'online' | 'error';
