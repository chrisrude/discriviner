// use crate::api::api_types;

// use songbird::id::{ChannelId, GuildId, UserId};
// use songbird::ConnectionInfo;

// pub struct Discrivener {
//     model: crate::old_model::Model,
// }

// impl Discrivener {
//     pub fn load(
//         model_path: String,
//         event_callback: std::sync::Arc<dyn Fn(api_types::VoiceChannelEvent) + Send + Sync>,
//         dump_everything_to_a_file: Option<String>,
//     ) -> Self {
//         Discrivener {
//             model: crate::old_model::Model::load(
//                 model_path,
//                 dump_everything_to_a_file,
//                 event_callback,
//             ),
//         }
//     }

//     /// Connect to a voice channel.
//     ///
//     /// channel_id:
//     /// ID of the voice channel to join
//     ///
//     /// This is not needed to establish a connection, but can be useful
//     /// for book-keeping.
//     /// URL of the voice websocket gateway server assigned to this call.
//     /// ID of the target voice channel's parent guild.
//     ///
//     /// Bots cannot connect to a guildless (i.e., direct message) voice call.
//     /// Unique string describing this session for validation/authentication purposes.
//     /// Ephemeral secret used to validate the above session.
//     /// UserID of this bot.
//     pub async fn connect(
//         &mut self,
//         channel_id: u64,
//         endpoint: &str,
//         guild_id: u64,
//         session_id: &str,
//         user_id: u64,
//         voice_token: &str,
//     ) -> Result<(), songbird::error::ConnectionError> {
//         let connection_info = ConnectionInfo {
//             channel_id: Some(ChannelId::from(channel_id)),
//             endpoint: endpoint.to_string(),
//             guild_id: GuildId::from(guild_id),
//             session_id: session_id.to_string(),
//             token: voice_token.to_string(),
//             user_id: UserId::from(user_id),
//         };
//         self.model.connect(connection_info).await
//     }

//     pub fn disconnect(&mut self) {
//         self.model.disconnect();
//     }
// }
