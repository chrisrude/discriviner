pub mod api {
    pub mod api_methods;
    pub mod api_types;
}
mod events {
    pub(crate) mod audio;
}
mod model {
    pub(crate) mod audio_manager;
    pub(crate) mod audio_slice;
    pub(crate) mod types;
    pub(crate) mod user_audio;
    pub(crate) mod voice_activity;
    pub(crate) mod whisper;
}
mod packet_handler;
