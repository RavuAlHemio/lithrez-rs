mod io_ext;
mod rez;


use std::fs::File;
use std::path::PathBuf;

use clap::Parser;


#[derive(Parser)]
struct Opts {
    pub rez_path: PathBuf,
}


fn output_rez_entries_recursive(entries: &[rez::Entry], indent: usize) {
    for entry in entries {
        for _ in 0..indent {
            print!("  ");
        }

        match entry {
            rez::Entry::Directory(dir) => {
                println!("{} ({})/", dir.name, dir.header.time);
                output_rez_entries_recursive(&dir.entries, indent + 1);
            },
            rez::Entry::Resource(res) => {
                print!("{}.{} [{}", res.name, res.extension, res.id);
                if res.description.len() > 0 {
                    print!(", {}", res.description);
                }
                println!("] ({}, {}+{} bytes)", res.header.time, res.header.position, res.header.size);
            },
        }
    }
}


fn main() {
    let opts = Opts::parse();

    let mut file = File::open(&opts.rez_path)
        .expect("failed to open REZ file");
    let rez_file = rez::File::try_read(&mut file)
        .expect("failed to read REZ directory");
    output_rez_entries_recursive(&rez_file.root_entries, 0);
}
