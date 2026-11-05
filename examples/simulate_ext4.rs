use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use aegisdisk::core::recovery::ext4::{
    Ext4Superblock, Ext4Inode, parse_ext4_directory_block,
    EXT4_SUPERBLOCK_OFFSET, EXT4_SUPERBLOCK_MAGIC,
};

fn main() {
    println!("=== AegisDisk EXT4 Simulation Test Suite ===");

    let img_path = "C:\\Users\\User\\.gemini\\antigravity-cli\\brain\\88d8057f-1b9f-40e8-9899-433334915c83\\scratch\\sim_ext4.img";
    let block_size = 4096u32;
    let total_blocks = 8192u32; // 32 MB volume
    let total_bytes = total_blocks as u64 * block_size as u64;

    // 1. Create simulated EXT4 disk image
    let mut f = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(img_path)
        .expect("Failed to create sim_ext4.img");
    f.set_len(total_bytes).expect("Failed to set image size");

    // 2. Write Superblock at offset 1024
    let mut sb_raw = vec![0u8; 1024];
    sb_raw[0x00..0x04].copy_from_slice(&2048u32.to_le_bytes()); // s_inodes_count
    sb_raw[0x04..0x08].copy_from_slice(&(total_blocks).to_le_bytes()); // s_blocks_count_lo
    sb_raw[0x0C..0x10].copy_from_slice(&7500u32.to_le_bytes()); // s_free_blocks_count_lo
    sb_raw[0x10..0x14].copy_from_slice(&2000u32.to_le_bytes()); // s_free_inodes_count
    sb_raw[0x14..0x18].copy_from_slice(&0u32.to_le_bytes()); // s_first_data_block (0 for 4KB blocks)
    sb_raw[0x18..0x1C].copy_from_slice(&2u32.to_le_bytes()); // s_log_block_size: 1024 << 2 = 4096
    sb_raw[0x20..0x24].copy_from_slice(&8192u32.to_le_bytes()); // s_blocks_per_group
    sb_raw[0x28..0x2C].copy_from_slice(&2048u32.to_le_bytes()); // s_inodes_per_group
    sb_raw[0x38..0x3A].copy_from_slice(&EXT4_SUPERBLOCK_MAGIC.to_le_bytes());
    sb_raw[0x58..0x5A].copy_from_slice(&256u16.to_le_bytes()); // s_inode_size
    sb_raw[0x78..0x88].copy_from_slice(b"LINUX_EXT4_TEST\0");

    f.seek(SeekFrom::Start(EXT4_SUPERBLOCK_OFFSET)).unwrap();
    f.write_all(&sb_raw).unwrap();

    // 3. Construct Inode Table at Block 10 (offset 40,960)
    let inode_table_block = 10u64;
    let inode_table_offset = inode_table_block * block_size as u64;

    // Test Case A: Active regular file (Inode 12)
    // Size: 12,288 bytes (3 blocks), uses extents pointing to blocks 100..102
    let mut inode_active = vec![0u8; 256];
    inode_active[0x00..0x02].copy_from_slice(&0x81A4u16.to_le_bytes()); // regular file 0644
    inode_active[0x04..0x08].copy_from_slice(&12288u32.to_le_bytes()); // 12 KB
    inode_active[0x14..0x18].copy_from_slice(&0u32.to_le_bytes()); // dtime = 0 (active)
    inode_active[0x1A..0x1C].copy_from_slice(&1u16.to_le_bytes()); // link count = 1
    inode_active[0x20..0x24].copy_from_slice(&0x0008_0000u32.to_le_bytes()); // EXT4_EXTENTS_FL

    let eh_pos = 0x28;
    inode_active[eh_pos..eh_pos + 2].copy_from_slice(&0xF30Au16.to_le_bytes()); // eh_magic
    inode_active[eh_pos + 2..eh_pos + 4].copy_from_slice(&1u16.to_le_bytes()); // eh_entries = 1
    inode_active[eh_pos + 4..eh_pos + 6].copy_from_slice(&4u16.to_le_bytes()); // eh_max = 4
    inode_active[eh_pos + 6..eh_pos + 8].copy_from_slice(&0u16.to_le_bytes()); // depth = 0 (leaf)

    let ext_pos = eh_pos + 12;
    inode_active[ext_pos..ext_pos + 4].copy_from_slice(&0u32.to_le_bytes()); // logical block 0
    inode_active[ext_pos + 4..ext_pos + 6].copy_from_slice(&3u16.to_le_bytes()); // 3 blocks
    inode_active[ext_pos + 6..ext_pos + 8].copy_from_slice(&0u16.to_le_bytes()); // start_hi
    inode_active[ext_pos + 8..ext_pos + 12].copy_from_slice(&100u32.to_le_bytes()); // start_lo = block 100

    // Test Case B: Deleted regular file (Inode 13)
    // Size: 8,192 bytes (2 blocks), deleted (dtime > 0, links = 0), extents pointing to blocks 200..201
    let mut inode_deleted = vec![0u8; 256];
    inode_deleted[0x00..0x02].copy_from_slice(&0x81A4u16.to_le_bytes()); // regular file
    inode_deleted[0x04..0x08].copy_from_slice(&8192u32.to_le_bytes()); // 8 KB
    inode_deleted[0x14..0x18].copy_from_slice(&1700000000u32.to_le_bytes()); // dtime = unix timestamp (deleted)
    inode_deleted[0x1A..0x1C].copy_from_slice(&0u16.to_le_bytes()); // links = 0
    inode_deleted[0x20..0x24].copy_from_slice(&0x0008_0000u32.to_le_bytes()); // EXT4_EXTENTS_FL

    inode_deleted[eh_pos..eh_pos + 2].copy_from_slice(&0xF30Au16.to_le_bytes());
    inode_deleted[eh_pos + 2..eh_pos + 4].copy_from_slice(&1u16.to_le_bytes());
    inode_deleted[eh_pos + 4..eh_pos + 6].copy_from_slice(&4u16.to_le_bytes());
    inode_deleted[eh_pos + 6..eh_pos + 8].copy_from_slice(&0u16.to_le_bytes());

    inode_deleted[ext_pos..ext_pos + 4].copy_from_slice(&0u32.to_le_bytes());
    inode_deleted[ext_pos + 4..ext_pos + 6].copy_from_slice(&2u16.to_le_bytes());
    inode_deleted[ext_pos + 6..ext_pos + 8].copy_from_slice(&0u16.to_le_bytes());
    inode_deleted[ext_pos + 8..ext_pos + 12].copy_from_slice(&200u32.to_le_bytes());

    // Write Inodes 12 & 13 to Inode Table (1-indexed, index 11 and 12)
    f.seek(SeekFrom::Start(inode_table_offset + 11 * 256)).unwrap();
    f.write_all(&inode_active).unwrap();
    f.write_all(&inode_deleted).unwrap();

    // 4. Construct Directory Block (Block 50, offset 204,800)
    let mut dir_block = vec![0u8; 4096];
    // Entry 1: "." -> Inode 2, rec_len 12
    dir_block[0..4].copy_from_slice(&2u32.to_le_bytes());
    dir_block[4..6].copy_from_slice(&12u16.to_le_bytes());
    dir_block[6] = 1; // name_len
    dir_block[7] = 2; // file_type = dir
    dir_block[8] = b'.';

    // Entry 2: ".." -> Inode 2, rec_len 12
    let o2 = 12;
    dir_block[o2..o2 + 4].copy_from_slice(&2u32.to_le_bytes());
    dir_block[o2 + 4..o2 + 6].copy_from_slice(&12u16.to_le_bytes());
    dir_block[o2 + 6] = 2; // name_len
    dir_block[o2 + 7] = 2; // file_type = dir
    dir_block[o2 + 8..o2 + 10].copy_from_slice(b"..");

    // Entry 3: "active_doc.pdf" -> Inode 12, rec_len 24
    let o3 = 24;
    dir_block[o3..o3 + 4].copy_from_slice(&12u32.to_le_bytes());
    dir_block[o3 + 4..o3 + 6].copy_from_slice(&24u16.to_le_bytes());
    dir_block[o3 + 6] = 14; // name_len
    dir_block[o3 + 7] = 1; // regular file
    dir_block[o3 + 8..o3 + 22].copy_from_slice(b"active_doc.pdf");

    // Entry 4: "deleted_report.docx" -> Inode 13, rec_len 4048 (spans rest of 4K block)
    let o4 = 48;
    dir_block[o4..o4 + 4].copy_from_slice(&13u32.to_le_bytes());
    dir_block[o4 + 4..o4 + 6].copy_from_slice(&(4096 - 48u16).to_le_bytes());
    dir_block[o4 + 6] = 19; // name_len
    dir_block[o4 + 7] = 1; // regular file
    dir_block[o4 + 8..o4 + 27].copy_from_slice(b"deleted_report.docx");

    f.seek(SeekFrom::Start(50 * block_size as u64)).unwrap();
    f.write_all(&dir_block).unwrap();

    // 5. Write file data payloads to physical blocks
    // Block 100 (active_doc.pdf)
    f.seek(SeekFrom::Start(100 * block_size as u64)).unwrap();
    f.write_all(b"%PDF-1.4\n1 0 obj\n<< /Title (Active Ext4 PDF Document) >>\nendobj\n%%EOF\n").unwrap();

    // Block 200 (deleted_report.docx)
    f.seek(SeekFrom::Start(200 * block_size as u64)).unwrap();
    f.write_all(b"PK\x03\x04\x14\x00\x00\x00\x08\x00[Content_Types].xml\x50\x4B\x05\x06deleted docx content").unwrap();

    f.flush().unwrap();
    println!("[1/4] Created synthetic 32MB EXT4 image with superblock, inode table, directory records, and payload blocks.");

    // TEST EXECUTION & VALIDATION
    // Step A: Parse Superblock
    let mut read_sb = vec![0u8; 1024];
    f.seek(SeekFrom::Start(EXT4_SUPERBLOCK_OFFSET)).unwrap();
    std::io::Read::read_exact(&mut f, &mut read_sb).unwrap();
    let sb = Ext4Superblock::parse(&read_sb).expect("Failed to parse EXT4 superblock");
    println!("[2/4] Superblock Validation:");
    println!("      - Volume Label:       '{}'", sb.volume_name);
    println!("      - Block Size:         {} bytes (log_size = 2)", sb.block_size);
    println!("      - Inode Size:         {} bytes", sb.inode_size);
    println!("      - Total Blocks:       {}", sb.blocks_count);
    println!("      - Free Blocks:        {}", sb.free_blocks_count);
    println!("      - Inodes Per Group:   {}", sb.inodes_per_group);

    // Step B: Directory Records Validation
    let mut read_dir = vec![0u8; 4096];
    f.seek(SeekFrom::Start(50 * block_size as u64)).unwrap();
    std::io::Read::read_exact(&mut f, &mut read_dir).unwrap();
    let dirents = parse_ext4_directory_block(&read_dir);
    println!("[3/4] Directory Traversal (Block 50):");
    for d in &dirents {
        println!("      * Inode {:2} | Type: {} | Name: '{}'", d.inode, d.file_type, d.name);
    }

    // Step C: Inode & Extent Mapping Validation
    let mut read_in12 = vec![0u8; 256];
    let mut read_in13 = vec![0u8; 256];
    f.seek(SeekFrom::Start(inode_table_offset + 11 * 256)).unwrap();
    std::io::Read::read_exact(&mut f, &mut read_in12).unwrap();
    std::io::Read::read_exact(&mut f, &mut read_in13).unwrap();

    let in_active = Ext4Inode::parse(&read_in12, 12).unwrap();
    let in_deleted = Ext4Inode::parse(&read_in13, 13).unwrap();

    println!("[4/4] Inode & Extent Reconstruction:");
    println!("      * Active File (Inode 12):");
    println!("        - Deleted Flag:     {}", in_active.is_deleted);
    println!("        - Size:             {} bytes", in_active.size_bytes);
    println!("        - Physical Blocks:  #{} (count: {})", in_active.extents[0].physical_block, in_active.extents[0].block_count);

    println!("      * Deleted File (Inode 13):");
    println!("        - Deleted Flag:     {} (dtime triggered)", in_deleted.is_deleted);
    println!("        - Size:             {} bytes", in_deleted.size_bytes);
    println!("        - Physical Blocks:  #{} (count: {})", in_deleted.extents[0].physical_block, in_deleted.extents[0].block_count);

    assert_eq!(sb.volume_name, "LINUX_EXT4_TEST");
    assert_eq!(dirents.len(), 4);
    assert!(!in_active.is_deleted);
    assert!(in_deleted.is_deleted);
    assert_eq!(in_deleted.extents[0].physical_block, 200);

    println!("\n>>> EXT4 SIMULATION SUITE: 100% PASS <<<");
}
