#[macro_use]
extern crate lazy_static;

use glib::{clone, MainLoop};
use gtk::prelude::*;

use gtk::{Application, ApplicationWindow, Box, Builder, Button, Entry, MenuItem, Window};

use pls::PlaylistElement;

use gstreamer::prelude::*;
use gstreamer::Element;

use std::{env, fs};
use fs::File;
use std::io::{Write};
use std::sync::{Arc, Mutex};
use std::thread;

lazy_static! {
    static ref CURRENT_STATION_PIPELINE: Arc<Mutex<Option<Element>>> = Arc::new(Mutex::new(None));
    static ref CURRENT_STATION_LOOP: Arc<Mutex<Option<MainLoop>>> = Arc::new(Mutex::new(None));
}

fn get_stations() -> Vec<Vec<PlaylistElement>> {
    let mut working_dir = env::current_dir().expect("Couldn't get current_dir");
    working_dir.push("stations");
    if !working_dir.exists() {
        match fs::create_dir(&working_dir) {
            Err(_) => {
                println!("Couldn't create stations directory maybe because it exists");
            },
            _ => {},
        }
    }
    if !working_dir.exists() {
        panic!("Couldn't create stations directory");
    }

    let mut stations: Vec<Vec<PlaylistElement>> = Vec::new();

    if let Ok(entries) = fs::read_dir(&working_dir) {
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

fn play_station(station: String) {
    gstreamer::init().expect("Couldn't init gst");

    let pipeline = gstreamer::parse_launch(&format!("playbin uri={}", station)).expect("Couldn't parse launch");

    let result = pipeline.set_state(gstreamer::State::Playing).expect("Couldn't set state to playing");
    let is_live = result == gstreamer::StateChangeSuccess::NoPreroll;

    let main_loop = glib::MainLoop::new(None, false);
    let main_loop_clone = main_loop.clone();
    let pipeline_weak = pipeline.downgrade();
    let bus = pipeline.bus().expect("Pipeline has no bus");

    bus.add_watch(move |_, msg| {
        let pipeline = match pipeline_weak.upgrade() {
            Some(pipeline) => pipeline,
            None => return glib::Continue(true),
        };
        let main_loop = &main_loop_clone;
        match msg.view() {
            gstreamer::MessageView::Error(err) => {
                println!(
                    "Error from {:?}: {} ({:?})",
                    err.src().map(|s| s.path_string()),
                    err.error(),
                    err.debug()
                );
                let _ = pipeline.set_state(gstreamer::State::Ready);
                main_loop.quit();
            }
            gstreamer::MessageView::Eos(..) => {
                // end-of-stream
                let _ = pipeline.set_state(gstreamer::State::Ready);
                main_loop.quit();
            }
            gstreamer::MessageView::Buffering(buffering) => {
                // If the stream is live, we do not care about buffering
                if is_live {
                    return glib::Continue(true);
                }

                let percent = buffering.percent();
                print!("Buffering ({}%)\r", percent);
                match std::io::stdout().flush() {
                    Ok(_) => {}
                    Err(err) => eprintln!("Failed: {}", err),
                };

                // Wait until buffering is complete before start/resume playing
                if percent < 100 {
                    let _ = pipeline.set_state(gstreamer::State::Paused);
                } else {
                    let _ = pipeline.set_state(gstreamer::State::Playing);
                }
            }
            gstreamer::MessageView::ClockLost(_) => {
                // Get a new clock
                let _ = pipeline.set_state(gstreamer::State::Paused);
                let _ = pipeline.set_state(gstreamer::State::Playing);
            }
            _ => (),
        }
        glib::Continue(true)
    })
    .expect("Failed to add bus watch");

    {
        let mut current_station_pipeline = CURRENT_STATION_PIPELINE.lock().expect("Couldn't lock current_station_pipeline");
        let mut current_station_loop = CURRENT_STATION_LOOP.lock().expect("Couldn't lock current_station_loop");
        *current_station_pipeline = Some(pipeline.clone());
        *current_station_loop = Some(main_loop.clone());
    }

    main_loop.run();

    bus.remove_watch().expect("Couldn't remove watch from bus");
    pipeline.set_state(gstreamer::State::Null).expect("Couldn't set state to null");
}

fn create_station(station_name: String, station_location: String) {
    let mut working_dir = env::current_dir().expect("Couldn't get current_dir");
    working_dir.push("stations");
    if !working_dir.exists() {
        match fs::create_dir(&working_dir) {
            Err(_) => {
                println!("Couldn't create stations directory maybe because it exists");
            },
            _ => {},
        }
    }
    if !working_dir.exists() {
        panic!("Couldn't create stations directory");
    }

    working_dir.push(station_name.to_lowercase().replace(" ", "_") + ".pls");

    pls::write(
        &[PlaylistElement {
            path: station_location,
            title: Some(station_name.clone()),
            len:  pls::ElementLength::Unknown,
        }],
        &mut File::create(working_dir).expect("Couldn't create station file")
    ).expect("Coulnd't write to station pls");
}

fn create_station_window(application: &Application, radio_station_box: &Box, play_pause_button: &Button) {
    let builder = Builder::from_string(include_str!("new_station.glade"));

    let window: Window = builder.object("new_station_window").expect("Couldn't get new_station_window");
    window.set_application(Some(application));
    window.set_title("Create New Station");

    let add: Button = builder.object("add_button").expect("Couldn't get add_button");
    let cancel: Button = builder.object("cancel_button").expect("Couldn't get cancel_button");

    let station_name_entry: Entry = builder.object("station_name_entry").expect("Couldn't get station_name_entry");
    let station_location_entry: Entry = builder.object("station_location_entry").expect("Couldn't get station_location_entry");

    add.connect_clicked(clone!{@weak window, @weak radio_station_box, @weak play_pause_button, @weak station_name_entry, @weak station_location_entry => move |_| {
        create_station(station_name_entry.text().to_string(), station_location_entry.text().to_string());

        let button = Button::with_label(&station_name_entry.text().to_string());
        button.connect_clicked(clone!{@weak play_pause_button => move |_| {
            let station = station_location_entry.text().to_string();
            let mut current_station_pipeline = CURRENT_STATION_PIPELINE.lock().expect("Couldn't lock current_station_pipeline");
            if let Some(pipeline) = &*current_station_pipeline {
                let _ = pipeline.set_state(gstreamer::State::Ready);
            }
            *current_station_pipeline = None;
            let mut current_station_loop = CURRENT_STATION_LOOP.lock().expect("Couldn't lock current_station_loop");
            if let Some(main_loop) = &*current_station_loop {
                main_loop.quit();
            }
            *current_station_loop = None;
            thread::spawn(move || {
                play_station(station);
            });
            play_pause_button.set_label("gtk-media-pause");
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

    let new: MenuItem = builder.object("new_menu_item").expect("Couldn't get new_menu_item");
    let quit: MenuItem = builder.object("quit_menu_item").expect("Couldn't get quit_menu_item");

    let radio_station_box: Box = builder.object("radio_station_box").expect("Couldn't get radio_station_box");

    let play_pause_button: Button = builder.object("play_pause_button").expect("Couldn't get play_pause_button");

    window.connect_destroy_with_parent_notify(move |_| {
        let mut current_station_loop = CURRENT_STATION_LOOP.lock().expect("Couldn't lock current_station_loop");
        if let Some(main_loop) = &*current_station_loop {
            main_loop.quit();
        }
        *current_station_loop = None;
    });

    new.connect_activate(clone!{@weak application, @weak radio_station_box, @weak play_pause_button => move |_| {
        create_station_window(&application, &radio_station_box, &play_pause_button);
    }});

    quit.connect_activate(clone!{@weak window => move |_| {
        window.close();
    }});

    play_pause_button.connect_clicked(clone!{@weak play_pause_button => move |_| {
        let label = play_pause_button.label().expect("Couldn't get play_pause_button's label");
        let will_pause = !label.as_str().eq("gtk-media-play");
        let current_station_pipeline = CURRENT_STATION_PIPELINE.lock().expect("Couldn't lock current_station_pipeline");
        if let Some(pipeline) = &*current_station_pipeline {
            if will_pause {
                let _ = pipeline.set_state(gstreamer::State::Paused);
                play_pause_button.set_label("gtk-media-play");
            } else {
                let _ = pipeline.set_state(gstreamer::State::Playing);
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
        let title = match &playlist_element.title {
            Some(t) => t,
            None => "Unknown",
        };
        let button = Button::with_label(title);
        button.connect_clicked(clone!{@weak play_pause_button => move |_| {
            let station = playlist_element.path.clone();
            let mut current_station_pipeline = CURRENT_STATION_PIPELINE.lock().expect("Couldn't lock current_station_pipeline");
            if let Some(pipeline) = &*current_station_pipeline {
                let _ = pipeline.set_state(gstreamer::State::Ready);
            }
            *current_station_pipeline = None;
            let mut current_station_loop = CURRENT_STATION_LOOP.lock().expect("Couldn't lock current_station_loop");
            if let Some(main_loop) = &*current_station_loop {
                main_loop.quit();
            }
            *current_station_loop = None;
            thread::spawn(move || {
                play_station(station);
            });
            play_pause_button.set_label("gtk-media-pause");
        }});
        radio_station_box.add(&button);
    }

    window.show_all();
}

fn main() {
    let application = Application::builder()
        .application_id("com.github.palaster.rust_radio")
        .build();

    application.connect_activate(|app| {
        build_ui(app)
    });

    application.run();
}
