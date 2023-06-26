fn main() {
    let spoken = discrivener::speak("Hello, world!");
    println!("Sample rate is {}", spoken.sample_rate);
}
