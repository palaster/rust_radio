#[macro_use]
extern crate lazy_static;

use eframe::egui::{CentralPanel, TopBottomPanel};
use eframe::{App, run_native};

use pls::PlaylistElement;

use std::fs::{self, File};
use std::io::{Write};
use std::sync::{mpsc::{self, Receiver, Sender}, Arc, Mutex};

use rodio::{Decoder, OutputStream, Sink};

const CHUNKS_BEFORE_START: u8 = 10;

lazy_static! {
    static ref SINK_SENDER: Arc<Mutex<Option<Sender<SinkCommands>>>> = Arc::new(Mutex::new(None));
}

enum SinkCommands {
    Start(String, String),
    Play,
    Pause,
    Quit,
}

#[derive(Default)]
struct Radio {
    is_playing: bool,
    current_station: Option<String>,
    creation_name: String,
    creation_url: String,
}

impl App for Radio {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        TopBottomPanel::top("controls").show(ctx, |ui| {
            if ui.button(if self.is_playing { "Pause" } else { "Play" }).clicked() {
                let sink_sender = SINK_SENDER.lock().expect("Couldn't lock SINK_SENDER");
                if let Some(sender) = &*sink_sender {
                    if self.is_playing {
                        match sender.send(SinkCommands::Pause) {
                            _ => {},
                        }
                    } else {
                        match sender.send(SinkCommands::Play) {
                            _ => {},
                        }
                    }
                    self.is_playing = !self.is_playing;
                }
            }
            ui.label(match &self.current_station {
                Some(station_name) => {
                    format!("Current Station: {}", station_name)
                },
                _ => {
                    "Station Not Selected".to_string()
                },
            });
        });
        TopBottomPanel::top("new_station").show(ctx, |ui| {
            ui.text_edit_singleline(&mut self.creation_name);
            ui.text_edit_singleline(&mut self.creation_url);
            if ui.button("Create Station").clicked() {
                if !self.creation_name.is_empty() && !self.creation_url.is_empty() {
                    create_station(self.creation_name.clone(), self.creation_url.clone());
                }
            }
        });
        CentralPanel::default().show(ctx, |ui| {
            for station in get_stations() {
                if station.len() != 1 {
                    println!("Only for use with streams");
                    break;
                }
                let playlist_element = station[0].clone();
                let title = match playlist_element.title {
                    Some(t) => t,
                    None => String::from("Unknown"),
                };
                if ui.button(title.clone()).clicked() {
                    let title = title.clone();
                    let station = playlist_element.path.clone();
                    let mut sink_sender = SINK_SENDER.lock().expect("Couldn't lock SINK_SENDER");
                    match &*sink_sender {
                        Some(sender) => {
                            match sender.send(SinkCommands::Start(title.clone(), station)) {
                                _ => {},
                            }
                        },
                        None => {
                            let (sender, receiver) = mpsc::channel();
                            let title_clone = title.clone();
                            tokio::spawn(async move {
                                start_ratio(receiver, title_clone, station).await;
                            });
                            *sink_sender = Some(sender);
                        },
                    }
                    self.is_playing = true;
                    self.current_station = Some(title);
                }
            }
        });
    }
}

fn get_stations() -> Vec<Vec<PlaylistElement>> {
    let mut audio_dir = dirs::audio_dir().expect("Couldn't get audio_dir");
    audio_dir.push("rust_radio");
    if !audio_dir.exists() {
        if let Err(_) = fs::create_dir(&audio_dir) {
            println!("Couldn't create rust_radio directory maybe because it exists");
        }
    }
    if !audio_dir.exists() {
        panic!("Couldn't create rust_radio directory");
    }

    let mut stations: Vec<Vec<PlaylistElement>> = Vec::new();

    if let Ok(entries) = fs::read_dir(&audio_dir) {
        for entry in entries {
            if let Ok(entry) = entry {
                if entry.file_type().expect("Couldn't get entry file type").is_file() {
                    if let Ok(file_name_as_string) = entry.file_name().into_string() {
                        if file_name_as_string.ends_with(".pls") {
                            stations.push(pls::parse(&mut File::open(entry.path()).expect("Couldn't open file")).expect("Couldn't parse playlist"));
                        }
                    }
                }
            }
        }
    }

    stations
}

async fn start_ratio(receiver: Receiver<SinkCommands>, name: String, url: String) {
    let (sink_sender, sink_receiver) = mpsc::channel::<SinkCommands>();

    std::thread::spawn(move || {
        let (_stream, stream_handle) = OutputStream::try_default().expect("Couldn't get default output stream");
        let mut sink = Sink::try_new(&stream_handle).expect("Couldn't create new sink from stream_handle");
        let mut path = None;
        let mut file;
        let mut source;
        loop {
            if let Ok(message) = sink_receiver.try_recv() {
                match message {
                    SinkCommands::Start(name, _) => {
                        sink.stop();
                        sink = match Sink::try_new(&stream_handle) {
                            Ok(t) => t,
                            _ => { continue; },
                        };
                        if let Some(old_path) = path {
                            match fs::remove_file(old_path) {
                                _ => {},
                            }
                        }
                        path = None;
                        let mut new_path = std::env::temp_dir();
                        new_path.push(&name);
                        file = match File::open(&new_path) {
                            Ok(t) => t,
                            Err(_) => {
                                println!("Couldn't open file {}", &name);
                                continue;
                            },
                        };
                        path = Some(new_path);
                        source = match Decoder::new(file) {
                            Ok(t) => t,
                            Err(_) => {
                                println!("Can't create decoder from file");
                                continue;
                            },
                        };
                        sink.play();
                        sink.append(source);
                    },
                    SinkCommands::Play => {
                        sink.play();
                    },
                    SinkCommands::Pause => {
                        sink.pause();
                    },
                    SinkCommands::Quit => {
                        return;
                    },
                }
            }
        }
    });

    let mut name = name.to_lowercase().replace(" ", "_");
    let mut url = url;
    let mut count_down = CHUNKS_BEFORE_START;
    let mut should_restart = true;

    loop {
        let mut path = std::env::temp_dir();
        path.push(&name);
        let mut file = File::create(path).expect(&format!("Couldn't create file {}", &name));

        let client = reqwest::Client::new();
        let mut response = client.get(&url).send().await.expect("Couldn't get response");
        while let Some(chunk) = response.chunk().await.expect("Couldn't get next chunk") {
            file.write(&chunk).expect("Couldn't write to file");
            if let Ok(message) = receiver.try_recv() {
                match message {
                    SinkCommands::Start(new_name, new_url) => {
                        let new_name = new_name.to_lowercase().replace(" ", "_");
                        if name == new_name && url == new_url {
                            continue;
                        }
                        name = new_name;
                        url = new_url;
                        count_down = CHUNKS_BEFORE_START;
                        should_restart = true;
                        break;
                    },
                    SinkCommands::Quit => {
                        sink_sender.send(message).unwrap();
                        return;
                    },
                    _ => {
                        sink_sender.send(message).unwrap();
                    },
                }
            }
            if should_restart {
                if count_down == 0 {
                    match sink_sender.send(SinkCommands::Start(name.clone(), url.clone())) {
                        _ => {},
                    }
                    should_restart = false;
                } else {
                    count_down -= 1;
                }
            }
        }
    }
}

fn create_station(name: String, url: String) {
    let mut audio_dir = dirs::audio_dir().expect("Couldn't get audio_dir");
    audio_dir.push("rust_radio");
    if !audio_dir.exists() {
        if let Err(_) = fs::create_dir(&audio_dir) {
            println!("Couldn't create rust_radio directory maybe because it exists");
        }
    }
    if !audio_dir.exists() {
        panic!("Couldn't create rust_radio directory");
    }

    audio_dir.push(name.to_lowercase().replace(" ", "_") + ".pls");

    pls::write(
        &[PlaylistElement {
            path: url,
            title: Some(name.clone()),
            len:  pls::ElementLength::Unknown,
        }],
        &mut File::create(audio_dir).expect("Couldn't create station file")
    ).expect("Coulnd't write to station pls");
}

#[tokio::main]
async fn main() {
    run_native("Radio Rust", eframe::NativeOptions::default(), Box::new(|_cc| Box::new(Radio::default())));
}
