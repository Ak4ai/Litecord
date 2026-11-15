# Third-party audio components

Litecord's Linux build includes the following open-source audio components.

## WebRTC AudioProcessing

- Upstream: https://webrtc.googlesource.com/src/ (packaged as
  `webrtc-audio-processing` 2.1)
- License: BSD 3-Clause
- Used for: AEC3 echo cancellation, noise suppression and AGC2.
- Copyright (c) 2011, The WebRTC project authors. All rights reserved.

## DeepFilterNet / libDF

- Upstream: https://github.com/Rikorose/DeepFilterNet
- License: MIT OR Apache-2.0
- Used for: the optional "Krisp-like (DeepFilterNet3)" noise suppression mode.
- The default DeepFilterNet3 ONNX model (`DeepFilterNet3_onnx.tar.gz`) is
  embedded in the binary, unmodified from upstream.

These components are independent of Discord/Krisp. Discord's Krisp noise
cancellation is proprietary and is not included in Litecord.
