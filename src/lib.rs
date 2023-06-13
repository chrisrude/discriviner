#![allow(dead_code)]
pub mod api {
    pub mod api_methods;
    pub mod api_types;
}
mod events {
    pub(crate) mod audio;
}
mod model {
    pub(crate) mod audio_buffer;
    pub(crate) mod types;
    pub(crate) mod voice_activity;
    pub(crate) mod whisper;
}
mod old_model;
mod packet_handler;
