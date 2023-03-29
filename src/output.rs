use std::{fs::{self, File}, sync::mpsc::Receiver};

use rodio::{OutputStream, Sink, Decoder};

pub(crate) enum OutputCommands {
    Start(String),
    Volume(f32),
    Play,
    Pause,
    Quit,
}

pub(crate) fn output(receiver: Receiver<OutputCommands>) {
    let (_stream, stream_handle) = OutputStream::try_default().expect("Couldn't get default output stream");
    let mut volume: f32 = 1.0;
    let mut sink = Sink::try_new(&stream_handle).expect("Couldn't create new sink from stream_handle");
    let mut has_started = false;
    let mut path_option = None;
    loop {
        if !has_started && path_option.is_some() {
            if let Some(path) = &path_option {
                if let Ok(file) = File::open(path) {
                    if let Ok(decoder) = Decoder::new(file) {
                        if let Ok(new_sink) = Sink::try_new(&stream_handle) {
                            sink = new_sink;
                            sink.set_volume(volume);
                            sink.append(decoder);
                            sink.play();
                            has_started = true;
                        }
                    }
                }
            }
        }
        if let Ok(message) = receiver.try_recv() {
            match message {
                OutputCommands::Start(file_name) => {
                    sink.stop();
                    has_started = false;
                    let mut new_path = std::env::temp_dir();
                    new_path.push(&file_name);
                    if let Some(old_path) = path_option {
                        if new_path != old_path && fs::remove_file(old_path).is_err() {
                        }
                    }
                    path_option = Some(new_path);
                },
                OutputCommands::Volume(new_volume) => {
                    if has_started && new_volume != sink.volume() {
                        volume = new_volume;
                        sink.set_volume(volume);
                    }
                },
                OutputCommands::Play => {
                    if has_started {
                        sink.play();
                    }
                },
                OutputCommands::Pause => {
                    if has_started {
                        sink.pause();
                    }
                },
                OutputCommands::Quit => {
                    return;
                },
            }
        }
    }
}