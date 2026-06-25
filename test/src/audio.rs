/// Parse an Ogg Opus file and extract individual Opus packets.
/// Returns packets where each packet is a valid Opus frame ready to send.
pub fn parse_ogg_opus(data: &[u8]) -> Result<Vec<Vec<u8>>, String> {
    let mut reader = ogg::PacketReader::new(std::io::Cursor::new(data));
    let mut packets = Vec::new();

    while let Ok(Some(packet)) = reader.read_packet() {
        // Each Ogg packet for Opus contains one complete Opus frame
        if packet.data.is_empty() {
            continue;
        }
        packets.push(packet.data.to_vec());
    }

    if packets.is_empty() {
        return Err("No Opus packets found in Ogg file".into());
    }

    Ok(packets)
}
