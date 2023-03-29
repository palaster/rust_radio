mod input;
mod output;

use output::OutputCommands;

use eframe::egui::{CentralPanel};
use eframe::epaint::Vec2;
use eframe::{App, run_native, NativeOptions};

use once_cell::sync::Lazy;
use pls::PlaylistElement;
use tokio::task::JoinHandle;

use std::fs::{self, File};
use std::sync::{mpsc::{self, Sender}, Mutex};

const EMPTY_STRING: &str = "";
const STONG_TITLE_ERROR: &str = "Error Please Try Again";

static OUTPUT_SENDER: Lazy<Mutex<Sender<OutputCommands>>> = Lazy::new(|| {
    let (sender, receiver) = mpsc::channel::<OutputCommands>();
    std::thread::spawn(move || {
        output::output(receiver);
    });
    Mutex::new(sender)
});
static SONG_TITLE: Lazy<Mutex<String>> = Lazy::new(|| Mutex::new(EMPTY_STRING.to_string()));

struct Radio {
    is_playing: bool,
    volume: f32,
    current_station: Option<String>,
    input_join_handle: Option<JoinHandle<()>>,
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
            input_join_handle: None,
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
                    let output_sender = OUTPUT_SENDER.lock().expect("Couldn't lock OUTPUT_SINK_SENDER");
                    if output_sender.send(if self.is_playing { OutputCommands::Pause } else { OutputCommands::Play }).is_err() {}
                    self.is_playing = !self.is_playing;
                }
                ui.add(eframe::egui::Slider::new(&mut self.volume, 0.0..=2.0).text("Volume"));
                {
                    let output_sink_sender = OUTPUT_SENDER.lock().expect("Couldn't lock OUTPUT_SINK_SENDER");
                    if output_sink_sender.send(OutputCommands::Volume(self.volume)).is_err() {}
                }
                ui.label(if let Some(station_name) = &self.current_station {
                    format!("Current Station: {}", station_name)
                } else {
                    String::from("Station Not Selected")
                });
                {
                    let song_title = SONG_TITLE.lock().expect("Couldn't lock SONG_TITLE");
                    if !song_title.is_empty() {
                        ui.label(song_title.to_string());
                    }
                }
                if self.is_creation_visible {
                    ui.text_edit_singleline(&mut self.creation_name);
                    ui.text_edit_singleline(&mut self.creation_url);
                    if ui.button("Create Station").clicked() && !self.creation_name.is_empty() && !self.creation_url.is_empty() {
                        create_station(self.creation_name.clone(), self.creation_url.clone());
                        self.is_creation_visible = false;
                        self.creation_name = String::from("Enter Station Name");
                        self.creation_url = String::from("Enter Station URL");
                    }
                    if ui.button("Cancel").clicked() {
                        self.is_creation_visible = false;
                        self.creation_name = String::from("Enter Station Name");
                        self.creation_url = String::from("Enter Station URL");
                    }
                } else if ui.button("Create New Station").clicked() {
                    self.is_creation_visible = true;
                }
                for station in get_stations() {
                    if station.len() != 1 {
                        println!("Only for use with streams");
                        continue;
                    }
                    let playlist_element = station[0].clone();
                    let title = match playlist_element.title {
                        Some(t) => t,
                        None => String::from("Unknown"),
                    };
                    if ui.button(title.clone()).clicked() {
                        let station = title.clone();
                        let url = playlist_element.path.clone();
                        if let Some(current_station) = &self.current_station {
                            if current_station == &station {
                                return;
                            }
                        }
                        let output_sink_sender = OUTPUT_SENDER.lock().expect("Couldn't lock OUTPUT_SINK_SENDER");
                        if output_sink_sender.send(OutputCommands::Pause).is_err() {}
                        if let Some(join_handle) = &self.input_join_handle {
                            join_handle.abort();
                        }
                        let station_clone = station.clone();
                        let url_clone = url.clone();
                        self.input_join_handle = Some(tokio::spawn(async move {
                            input::input(station_clone, url_clone).await;
                        }));
                        self.is_playing = true;
                        self.current_station = Some(station);
                        let mut song_title = SONG_TITLE.lock().expect("Couldn't lock SONG_TITLE");
                        *song_title = EMPTY_STRING.to_string();
                    }
                }
            });
        });
        if self.is_playing {
            if self.input_join_handle.is_none() {
                self.is_playing = false;
            } else if let Some(join_handle) = &self.input_join_handle {
                self.is_playing = !join_handle.is_finished();
            }
        }
    }

    fn on_close_event(&mut self) -> bool {
        if let Some(join_handle) = &self.input_join_handle {
            join_handle.abort();
        }
        self.input_join_handle = None;
        let output_sink_sender = OUTPUT_SENDER.lock().expect("Couldn't lock SINK_SENDER");
        if output_sink_sender.send(OutputCommands::Quit).is_err() {}
        true
    }
}

fn get_stations() -> Vec<Vec<PlaylistElement>> {
    let mut audio_dir = dirs::audio_dir().expect("Couldn't get audio_dir");
    audio_dir.push("rust_radio");
    if !audio_dir.exists() && fs::create_dir(&audio_dir).is_err() {
        println!("Couldn't create rust_radio directory maybe because it exists");
    }
    if !audio_dir.exists() {
        panic!("Couldn't create rust_radio directory");
    }

    let mut stations: Vec<Vec<PlaylistElement>> = Vec::new();

    if let Ok(entries) = fs::read_dir(&audio_dir) {
        for entry in entries.flatten() {
            if entry.file_type().expect("Couldn't get entry file type").is_file() {
                if let Ok(file_name_as_string) = entry.file_name().into_string() {
                    if file_name_as_string.ends_with(".pls") {
                        stations.push(pls::parse(&mut File::open(entry.path()).expect("Couldn't open file")).expect("Couldn't parse playlist"));
                    }
                }
            }
        }
    }

    stations
}

fn create_station(name: String, url: String) {
    let mut audio_dir = dirs::audio_dir().expect("Couldn't get audio_dir");
    audio_dir.push("rust_radio");
    if !audio_dir.exists() && fs::create_dir(&audio_dir).is_err() {
        println!("Couldn't create rust_radio directory maybe because it exists");
    }
    if !audio_dir.exists() {
        panic!("Couldn't create rust_radio directory");
    }

    audio_dir.push(name.to_lowercase().replace(' ', "_") + ".pls");

    pls::write(
        &[PlaylistElement {
            path: url,
            title: Some(name),
            len:  pls::ElementLength::Unknown,
        }],
        &mut File::create(audio_dir).expect("Couldn't create station file")
    ).expect("Coulnd't write to station pls");
}

#[tokio::main]
async fn main() {
    let app_options = NativeOptions { initial_window_size: Some(Vec2::new(320.0, 128.0)), ..Default::default() };
    run_native("Radio Rust", app_options, Box::new(|_cc| Box::<Radio>::default()));
}
