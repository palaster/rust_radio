use std::{fs::File, io::Write};

use crate::{OUTPUT_SENDER, SONG_TITLE, STONG_TITLE_ERROR, output::OutputCommands};

const CHUNKS_BEFORE_START: u8 = 20;

pub(crate) async fn input(name: String, url: String) {
    let working_name = name.to_lowercase().replace(' ', "_");
    let working_url = url;
    let mut count_down = CHUNKS_BEFORE_START;
    let mut should_restart = true;

    let client = reqwest::Client::new();

    loop {
        let mut response = match client.get(&working_url).header("icy-metadata", "1").send().await {
            Ok(t) => t,
            Err(_) => {
                let output_sink_sender = OUTPUT_SENDER.lock().expect("Couldn't lock INNER_SINK_SENDER");
                if output_sink_sender.send(OutputCommands::Pause).is_err() {}
                let mut song_title = SONG_TITLE.lock().expect("Couldn't lock SONG_TITLE");
                *song_title = STONG_TITLE_ERROR.to_string();
                return;
            },
        };
        if let Some(header_value) = response.headers().get("content-type") {
            if header_value.to_str().unwrap_or_default() != "audio/mpeg" {
                let output_sink_sender = OUTPUT_SENDER.lock().expect("Couldn't lock INNER_SINK_SENDER");
                if output_sink_sender.send(OutputCommands::Pause).is_err() {}
                let mut song_title = SONG_TITLE.lock().expect("Couldn't lock SONG_TITLE");
                *song_title = STONG_TITLE_ERROR.to_string();
                return;
            }
        } else {
            let output_sink_sender = OUTPUT_SENDER.lock().expect("Couldn't lock INNER_SINK_SENDER");
            if output_sink_sender.send(OutputCommands::Pause).is_err() {}
            let mut song_title = SONG_TITLE.lock().expect("Couldn't lock SONG_TITLE");
            *song_title = STONG_TITLE_ERROR.to_string();
            return;
        }
        let meta_interval: usize = if let Some(header_value) = response.headers().get("icy-metaint") {
            header_value.to_str().unwrap_or_default().parse().unwrap_or_default()
        } else {
            0
        };
        let mut path = std::env::temp_dir();
        path.push(&working_name);
        let mut file = File::create(path).unwrap_or_else(|_| panic!("Couldn't create file {}", &working_name));
        let mut counter = meta_interval;
        let mut awaiting_metadata_size = false;
        let mut metadata_size: u8 = 0;
        let mut awaiting_metadata = false;
        let mut metadata: Vec<u8> = Vec::new();
        while let Some(chunk) = response.chunk().await.expect("Couldn't get next chunk") {
            for byte in &chunk {
                if meta_interval != 0 {
                    if awaiting_metadata_size {
                        awaiting_metadata_size = false;
                        metadata_size = *byte * 16;
                        if metadata_size == 0 {
                            counter = meta_interval;
                        } else {
                            awaiting_metadata = true;
                        }
                    } else if awaiting_metadata {
                        metadata.push(*byte);
                        metadata_size = metadata_size.saturating_sub(1);
                        if metadata_size == 0 {
                            awaiting_metadata = false;
                            let metadata_string = std::str::from_utf8(&metadata).unwrap_or("");
                            if !metadata_string.is_empty() {
                                const STREAM_TITLE_KEYWORD: &str = "StreamTitle='";
                                if let Some(index) = metadata_string.find(STREAM_TITLE_KEYWORD) {
                                    let left_index = index + 13;
                                    let stream_title_substring = &metadata_string[left_index..];
                                    if let Some(right_index) = stream_title_substring.find('\'') {
                                        let trimmed_song_title = &stream_title_substring[..right_index];
                                        let mut song_title = SONG_TITLE.lock().expect("Couldn't lock SONG_TITLE");
                                        *song_title = format!("Current Song: {}", trimmed_song_title);
                                    }
                                }
                            }
                            metadata.clear();
                            counter = meta_interval;
                        }
                    } else {
                        file.write_all(&[*byte]).expect("Couldn't write to file");
                        counter = counter.saturating_sub(1);
                        if counter == 0 {
                            awaiting_metadata_size = true;
                        }
                    }
                } else {
                    file.write_all(&[*byte]).expect("Couldn't write to file");
                }
            }
            if should_restart {
                if count_down == 0 {
                    let output_sink_sender = OUTPUT_SENDER.lock().expect("Couldn't lock INNER_SINK_SENDER");
                    if output_sink_sender.send(OutputCommands::Start(working_name.clone())).is_err() {
                        let mut song_title = SONG_TITLE.lock().expect("Couldn't lock SONG_TITLE");
                        *song_title = STONG_TITLE_ERROR.to_string();
                        return;
                    }
                    should_restart = false;
                } else {
                    count_down -= 1;
                }
            }
        }
    }
}