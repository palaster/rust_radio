#[macro_use]
extern crate lazy_static;

use eframe::egui::{CentralPanel};
use eframe::epaint::Vec2;
use eframe::{App, run_native, NativeOptions};

use pls::PlaylistElement;

use std::fs::{self, File};
use std::io::{Write};
use std::sync::{mpsc::{self, Receiver, Sender}, Arc, Mutex};

use rodio::{Decoder, OutputStream, Sink};

const CHUNKS_BEFORE_START: u8 = 10;

lazy_static! {
    static ref OUTER_SINK_SENDER: Arc<Mutex<Option<Sender<SinkCommands>>>> = Arc::new(Mutex::new(None));
    static ref INNER_SINK_SENDER: Arc<Mutex<Option<Sender<SinkCommands>>>> = Arc::new(Mutex::new(None));
    static ref SONG_TITLE: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
}

enum SinkCommands {
    Start(String, String),
    Volume(f32),
    Play,
    Pause,
    Quit,
}

struct Radio {
    is_playing: bool,
    volume: f32,
    current_station: Option<String>,
    is_creation_visible: bool,
    creation_name: String,
    creation_url: String,
}

impl Default for Radio {
    fn default() -> Self {
        Radio {
            is_playing: false,
            volume: 1.0,
            current_station: None,
            is_creation_visible: false,
            creation_name: String::from("Enter Station Name"),
            creation_url: String::from("Enter Station URL"),
        }
    }    
}

impl App for Radio {
    fn update(&mut self, ctx: &eframe::egui::Context, _frame: &mut eframe::Frame) {
        CentralPanel::default().show(ctx, |ui| {
            ui.vertical_centered_justified(|ui| {
                if ui.button(if self.is_playing { "Pause" } else { "Play" }).clicked() {
                    let sink_sender = OUTER_SINK_SENDER.lock().expect("Couldn't lock SINK_SENDER");
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
                ui.add(eframe::egui::Slider::new(&mut self.volume, 0.0..=2.0).text("Volume"));
                {
                    let inner_sink_sender = INNER_SINK_SENDER.lock().expect("Couldn't lock SINK_SENDER");
                    if let Some(sender) = &*inner_sink_sender {
                        sender.send(SinkCommands::Volume(self.volume)).unwrap();
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
                {
                    let song_title = SONG_TITLE.lock().expect("Couldn't lock SONG_TITLE");
                    if song_title.is_some() {
                        ui.label(format!("Current Song: {}", song_title.clone().expect("")));
                    }
                }
                if self.is_creation_visible {
                    ui.text_edit_singleline(&mut self.creation_name);
                    ui.text_edit_singleline(&mut self.creation_url);
                    if ui.button("Create Station").clicked() {
                        if !self.creation_name.is_empty() && !self.creation_url.is_empty() {
                            create_station(self.creation_name.clone(), self.creation_url.clone());
                            self.is_creation_visible = false;
                            self.creation_name = String::from("Enter Station Name");
                            self.creation_url = String::from("Enter Station URL");
                        }
                    }
                    if ui.button("Cancel").clicked() {
                        self.is_creation_visible = false;
                        self.creation_name = String::from("Enter Station Name");
                        self.creation_url = String::from("Enter Station URL");
                    }
                } else {
                    if ui.button("Create New Station").clicked() {
                        self.is_creation_visible = true;
                    }
                }
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
                        let mut sink_sender = OUTER_SINK_SENDER.lock().expect("Couldn't lock SINK_SENDER");
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
        });
    }

    fn on_close_event(&mut self) -> bool {
        let mut sink_sender = OUTER_SINK_SENDER.lock().expect("Couldn't lock SINK_SENDER");
        if let Some(sender) = &*sink_sender {
            sender.send(SinkCommands::Quit).unwrap();
        }
        *sink_sender = None;
        true
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
                    SinkCommands::Volume(new_volume) => {
                        if new_volume != sink.volume() {
                            sink.set_volume(new_volume);
                        }
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

    {
        let mut inner_sink_sender = INNER_SINK_SENDER.lock().expect("Couldn't lock INNER_SINK_SENDER");
        *inner_sink_sender = Some(sink_sender);
    }

    let mut name = name.to_lowercase().replace(" ", "_");
    let mut url = url;
    let mut count_down = CHUNKS_BEFORE_START;
    let mut should_restart = true;

    loop {
        let mut path = std::env::temp_dir();
        path.push(&name);
        let mut file = File::create(path).expect(&format!("Couldn't create file {}", &name));

        let client = reqwest::Client::new();
        let mut response = client.get(&url).header("icy-metadata", "1").send().await.expect("Couldn't get response");
        let meta_interval: usize = match response.headers().get("icy-metaint") {
            Some(t) => t.to_str().unwrap_or_default().parse().unwrap_or_default(),
            _ => 0,
        };
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
                                match (metadata_string.find("'"), metadata_string.rfind("'")) {
                                    (Some(left_index), Some(right_index)) => {
                                        let trimmed_song_title = &metadata_string[(left_index + 1)..right_index];
                                        let mut song_title = SONG_TITLE.lock().expect("Couldn't lock SONG_TITLE");
                                        *song_title = Some(trimmed_song_title.to_owned());
                                    },
                                    _ => {},
                                }
                            }
                            metadata.clear();
                            counter = meta_interval;
                        }
                    } else {
                        file.write(&[*byte]).expect("Couldn't write to file");
                        counter = counter.saturating_sub(1);
                        if counter == 0 {
                            awaiting_metadata_size = true;
                        }
                    }
                } else {
                    file.write(&[*byte]).expect("Couldn't write to file");
                }
            }
            if let Ok(message) = receiver.try_recv() {
                match message {
                    SinkCommands::Start(new_name, new_url) => {
                        let new_name = new_name.to_lowercase().replace(" ", "_");
                        if name == new_name && url == new_url {
                            continue;
                        }
                        name = new_name;
                        url = new_url;
                        let mut song_title = SONG_TITLE.lock().expect("Couldn't lock SONG_TITLE");
                        *song_title = None;
                        count_down = CHUNKS_BEFORE_START;
                        should_restart = true;
                        let inner_sink_sender = INNER_SINK_SENDER.lock().expect("Couldn't lock INNER_SINK_SENDER");
                        if let Some(sink_sender) = &*inner_sink_sender {
                            sink_sender.send(SinkCommands::Pause).unwrap();
                        }
                        break;
                    },
                    SinkCommands::Quit => {
                        let inner_sink_sender = INNER_SINK_SENDER.lock().expect("Couldn't lock INNER_SINK_SENDER");
                        if let Some(sink_sender) = &*inner_sink_sender {
                            sink_sender.send(message).unwrap();
                        }
                        return;
                    },
                    _ => {
                        let inner_sink_sender = INNER_SINK_SENDER.lock().expect("Couldn't lock INNER_SINK_SENDER");
                        if let Some(sink_sender) = &*inner_sink_sender {
                            sink_sender.send(message).unwrap();
                        }
                    },
                }
            }
            if should_restart {
                if count_down == 0 {
                    let inner_sink_sender = INNER_SINK_SENDER.lock().expect("Couldn't lock INNER_SINK_SENDER");
                    if let Some(sink_sender) = &*inner_sink_sender {
                        sink_sender.send(SinkCommands::Start(name.clone(), url.clone())).unwrap();
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
    let mut app_options = NativeOptions::default();
    app_options.initial_window_size = Some(Vec2::new(320.0, 128.0));
    run_native("Radio Rust", app_options, Box::new(|_cc| Box::new(Radio::default())));
}
