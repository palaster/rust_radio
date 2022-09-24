#[macro_use]
extern crate lazy_static;

use glib::{clone};
use gtk::prelude::*;

use gtk::{Application, ApplicationWindow, Box, Builder, Button, Entry, Label, Window};

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

fn refresh_stations(radio_station_box: &Box, play_pause_button: &Button, current_station_label: &Label) {
    radio_station_box.foreach(|widget| unsafe {
        widget.destroy()
    });

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
        let button = Button::with_label(&title);
        button.connect_clicked(clone!{@weak play_pause_button, @weak current_station_label => move |_| {
            let title = title.clone();
            let station = playlist_element.path.clone();
            let mut sink_sender = SINK_SENDER.lock().expect("Couldn't lock SINK_SENDER");
            match &*sink_sender {
                Some(sender) => {
                    sender.send(SinkCommands::Start(title.clone(), station)).unwrap();
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
            play_pause_button.set_label("gtk-media-pause");
            current_station_label.set_label(&(String::from("Current Station ") + &title));
        }});
        button.show();
        radio_station_box.add(&button);
    }
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

fn create_station_window(application: &Application, radio_station_box: &Box, play_pause_button: &Button, current_station_label: &Label) {
    let builder = Builder::from_string(include_str!("new_station.glade"));

    let window: Window = builder.object("new_station_window").expect("Couldn't get new_station_window");
    window.set_application(Some(application));
    window.set_title("Create New Station");

    let add: Button = builder.object("add_button").expect("Couldn't get add_button");
    let cancel: Button = builder.object("cancel_button").expect("Couldn't get cancel_button");

    let station_name_entry: Entry = builder.object("station_name_entry").expect("Couldn't get station_name_entry");
    let station_location_entry: Entry = builder.object("station_location_entry").expect("Couldn't get station_location_entry");

    add.connect_clicked(clone!{@weak window, @weak radio_station_box, @weak play_pause_button, @weak current_station_label, @weak station_name_entry, @weak station_location_entry => move |_| {
        create_station(station_name_entry.text().to_string(), station_location_entry.text().to_string());

        let button = Button::with_label(&station_name_entry.text().to_string());
        button.connect_clicked(clone!{@weak play_pause_button, @weak current_station_label => move |_| {
            let title = station_name_entry.text().to_string();
            let station = station_location_entry.text().to_string();
            let mut sink_sender = SINK_SENDER.lock().expect("Couldn't lock SINK_SENDER");
            match &*sink_sender {
                Some(sender) => {
                    sender.send(SinkCommands::Start(title.clone(), station)).unwrap();
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
            play_pause_button.set_label("gtk-media-pause");
            current_station_label.set_label(&(String::from("Current Station ") + &title));
        }});
        button.show();
        radio_station_box.add(&button);

        window.close();
    }});

    cancel.connect_clicked(clone!{@weak window => move |_| {
        window.close();
    }});

    window.show_all();
}

fn build_ui(application: &gtk::Application) {
    let builder = Builder::from_string(include_str!("rust_radio.glade"));

    let window: ApplicationWindow = builder.object("main_application_window").expect("Couldn't get main_application_window");
    window.set_application(Some(application));
    window.set_title("Radio Rust");

    let new_button: Button = builder.object("new_button").expect("Couldn't get new_button");
    let refresh_button: Button = builder.object("refresh_button").expect("Couldn't get refresh_button");
    let close_button: Button = builder.object("close_button").expect("Couldn't get close_button");

    let current_station_label: Label = builder.object("current_station_label").expect("Couldn't get current_station_label");

    let radio_station_box: Box = builder.object("radio_station_box").expect("Couldn't get radio_station_box");

    let play_pause_button: Button = builder.object("play_pause_button").expect("Couldn't get play_pause_button");

    window.connect_destroy_with_parent_notify(move |_| {
        let mut sink_sender = SINK_SENDER.lock().expect("Couldn't lock SINK_SENDER");
        if let Some(sender) = &*sink_sender {
            sender.send(SinkCommands::Quit).unwrap();
        }
        *sink_sender = None;
    });

    new_button.connect_clicked(clone!{@weak application, @weak radio_station_box, @weak play_pause_button, @weak current_station_label => move |_| {
        create_station_window(&application, &radio_station_box, &play_pause_button, &current_station_label);
    }});

    refresh_button.connect_clicked(clone!{@weak radio_station_box, @weak play_pause_button, @weak current_station_label => move |_| {
        refresh_stations(&radio_station_box, &play_pause_button, &current_station_label);
    }});

    close_button.connect_clicked(clone!{@weak window => move |_| {
        window.close();
    }});

    play_pause_button.connect_clicked(clone!{@weak play_pause_button => move |_| {
        let label = play_pause_button.label().expect("Couldn't get play_pause_button's label");
        let will_pause = !label.as_str().eq("gtk-media-play");
        let sink_sender = SINK_SENDER.lock().expect("Couldn't lock SINK_SENDER");
        if let Some(sender) = &*sink_sender {
            if will_pause {
                sender.send(SinkCommands::Pause).unwrap();
                play_pause_button.set_label("gtk-media-play");
            } else {
                sender.send(SinkCommands::Play).unwrap();
                play_pause_button.set_label("gtk-media-pause");
            }
        }
    }});

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
        let button = Button::with_label(&title);
        button.connect_clicked(clone!{@weak play_pause_button, @weak current_station_label => move |_| {
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
            play_pause_button.set_label("gtk-media-pause");
            current_station_label.set_label(&(String::from("Current Station ") + &title));
        }});
        radio_station_box.add(&button);
    }

    window.show_all();
}

#[tokio::main]
async fn main() {
    let application = Application::builder()
        .application_id("com.github.palaster.rust_radio")
        .build();

    application.connect_activate(|app| {
        build_ui(app)
    });

    application.run();
}
