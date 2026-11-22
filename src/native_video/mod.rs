//! Native Discord Go Live transmission.
//!
//! Additive to the `LTPV` peer-to-peer path: when the user turns on "Go Live"
//! in the share dialog, the frames the screen encoder already produces are also
//! packetized as RTP/H.264 and sent to Discord's stream server, so official
//! clients can watch.  No extra encode happens and nothing runs while off.

pub mod crypto;
pub mod h264;
pub mod rtcp;
pub mod rtp;
pub mod sender;
pub mod stream;

pub use sender::{is_broadcasting, offer_outbound_frame};
