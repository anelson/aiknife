//! Implementation of audio-related AI functionality (presently Whisper)

mod device;
pub use device::*;

// TODO:
// - Streaming audio from a device or file
// - List all audio devices
// - Code to support streaming transcription (only from a live device unless we're testing with an
// audio file)
//   - Initially take a minimum of one second of audio and pass it to Whisper
//   - Do the Whisper inference in a separate task so that we can keep buffering audio
//   - Apply back-pressure so if inference takes more than one second we keep buffering and run
//   inference again as soon as we can
//   - Keep track of the output tokens; if outputs are repeated in N inference steps (default is 2)
//   then they are confirmed, otherwise they are just potential tokens
//   - Figure out where to step the audio and start again.  This will involve using VAD, but it's
//   not as simple as chopping a segment whenever the VAD indicates lack of voice.  Until the user
//   is done talking, we want to keep processing the audio up to the last 30 seconds.  Once we have
//   more than 30 seconds of audio, then we need to truncate and for that we'll use VAD.
//   - Later I want to add some filters like DeepNetFilter and possibly something to normalize
//   volume
//   - Disable Whisper timestamps they are useless
//   - Feed previous text back into the context
//   - Run VAD on the input as a way to detect when thre is no new voice since we last ran
//   inference; that's a performance/battery life optimization.  When the user starts to talk
//   again, we'll still feed in up to the last 30 seconds of audio, but only actual voice; long
//   pauses should be filtered out.
//
