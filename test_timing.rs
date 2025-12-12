use fugue::*;

fn main() {
    let sample_rate = 44100;
    let tempo = Tempo::new(120.0);
    
    println!("Testing timing calculation:");
    println!("Sample rate: {}", sample_rate);
    println!("Tempo: {} BPM", tempo.get_bpm());
    
    let samples_per_beat = tempo.samples_per_beat(sample_rate);
    println!("Samples per beat: {}", samples_per_beat);
    
    let note_duration = 0.25; // Quarter note
    let samples_per_note = (samples_per_beat * note_duration) as u64;
    println!("Quarter note (0.25 beats): {} samples = {:.3}s", 
        samples_per_note, samples_per_note as f64 / sample_rate as f64);
    
    let note_duration = 0.125; // Eighth note
    let samples_per_note = (samples_per_beat * note_duration) as u64;
    println!("Eighth note (0.125 beats): {} samples = {:.3}s", 
        samples_per_note, samples_per_note as f64 / sample_rate as f64);
    
    println!("\nExpected values:");
    println!("At 120 BPM: 1 beat = 0.5 seconds");
    println!("Quarter note = 0.5s");
    println!("Eighth note = 0.25s");
}
