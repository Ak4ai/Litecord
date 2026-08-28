fn generate_fec_parity(chunks: &[&[u8]]) -> Vec<u8> {
    if chunks.is_empty() { return Vec::new(); }
    let max_len = chunks.iter().map(|c| c.len()).max().unwrap_or(0);
    let mut parity = vec![0u8; max_len];
    for chunk in chunks {
        for (i, &b) in chunk.iter().enumerate() {
            parity[i] ^= b;
        }
    }
    parity
}

fn recover_lost_chunk(received_chunks: &[(u16, Vec<u8>)], parity: &[u8], missing_idx: u16, total_chunks: u16, total_frame_len: usize, chunk_size: usize) -> Vec<u8> {
    let expected_len = if missing_idx == total_chunks - 1 {
        let rem = total_frame_len % chunk_size;
        if rem == 0 { chunk_size } else { rem }
    } else {
        chunk_size
    };

    let mut recovered = parity.to_vec();
    if recovered.len() < expected_len {
        recovered.resize(expected_len, 0);
    }
    for (_, chunk) in received_chunks {
        for (i, &b) in chunk.iter().enumerate() {
            if i < recovered.len() {
                recovered[i] ^= b;
            }
        }
    }
    recovered.truncate(expected_len);
    recovered
}

fn main() {
    let chunk_size = 1300;
    // A 3200-byte frame split into 3 chunks: 1300, 1300, 600
    let mut frame_data = vec![0u8; 3200];
    for i in 0..3200 { frame_data[i] = (i % 251) as u8; }

    let chunks: Vec<&[u8]> = frame_data.chunks(chunk_size).collect();
    let parity = generate_fec_parity(&chunks);

    println!("Total frame: {} bytes in {} chunks", frame_data.len(), chunks.len());

    // Test dropping chunk 0
    let received_without_0 = vec![(1, chunks[1].to_vec()), (2, chunks[2].to_vec())];
    let recovered_0 = recover_lost_chunk(&received_without_0, &parity, 0, 3, 3200, chunk_size);
    assert_eq!(recovered_0, chunks[0]);
    println!("✅ Chunk 0 (1300 bytes) recuperado com sucesso!");

    // Test dropping chunk 1
    let received_without_1 = vec![(0, chunks[0].to_vec()), (2, chunks[2].to_vec())];
    let recovered_1 = recover_lost_chunk(&received_without_1, &parity, 1, 3, 3200, chunk_size);
    assert_eq!(recovered_1, chunks[1]);
    println!("✅ Chunk 1 (1300 bytes) recuperado com sucesso!");

    // Test dropping chunk 2 (the shorter tail chunk of 600 bytes)
    let received_without_2 = vec![(0, chunks[0].to_vec()), (1, chunks[1].to_vec())];
    let recovered_2 = recover_lost_chunk(&received_without_2, &parity, 2, 3, 3200, chunk_size);
    assert_eq!(recovered_2, chunks[2]);
    println!("✅ Chunk 2 (600 bytes tail) recuperado com sucesso!");
}
