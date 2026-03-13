fn main() -> Result<(), Box<dyn std::error::Error>> {
    tonic_build::configure().compile(
        &["../../ts/client/scripts/execution-queue/ctm_sequencer.proto"],
        &["../../ts/client/scripts/execution-queue"],
    )?;
    Ok(())
}
