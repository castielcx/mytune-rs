//! Records 3 s from the default microphone as a Discord voice message and prints what came out; nothing is
//! saved or sent. `.\build.ps1 run -p mytune-chat --example voicenote`
use mytune_chat::yapfast_core::voice_note::{seconds_of, Recorder};

fn main() {
    // the engine's own input meter on the same microphone, to tell a silent microphone from a wrong reading
    let meter = mytune_chat::yapfast_core::meter::InputMeter::start(None);
    std::thread::sleep(std::time::Duration::from_millis(1000));
    println!("meter level={:.3} failed={}", meter.level(), meter.failed());
    drop(meter);
    let rec = Recorder::start(None);
    for _ in 0..6 {
        std::thread::sleep(std::time::Duration::from_millis(500));
        println!("failed={} seconds={:.2} level={:.3}", rec.failed(), rec.seconds(), rec.level());
    }
    match rec.finish() {
        Some(r) => {
            println!("ogg={} bytes, seconds={:.2}, waveform={} bytes (max {}), file says {:.2} s", r.ogg.len(), r.seconds, r.waveform.len(), r.waveform.iter().max().unwrap_or(&0), seconds_of(&r.ogg));
            // plays it back on the default speakers, to see the decoder take the file
            let player = mytune_chat::yapfast_core::voice_note::Player::start(r.ogg, None);
            let from = std::time::Instant::now();
            while !player.ended() && from.elapsed().as_secs() < 8 {
                std::thread::sleep(std::time::Duration::from_millis(100));
            }
            println!("played {:.2} s in {:.1} s, ended={}", player.seconds(), from.elapsed().as_secs_f32(), player.ended());
        }
        None => println!("no recording"),
    }
}
