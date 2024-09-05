mod io_ext;
mod rez;


use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use clap::Parser;
use regex::Regex;


#[derive(Parser)]
enum Mode {
    /// List the contents of a REZ file.
    List(ListOpts),

    /// Extract the files in a REZ file to a specific directory.
    Extract(ExtractOpts),
}

#[derive(Parser)]
struct ListOpts {
    /// The REZ file whose contents to list.
    pub rez_file: PathBuf,
}

#[derive(Parser)]
struct ExtractOpts {
    /// If given, limits the the extraction to files whose paths match this pattern. If given
    /// multiple times, a file will be extracted if it matches at least one of the patterns.
    #[arg(short, long = "filter")]
    pub filters: Vec<String>,

    /// The REZ file whose contents to extract.
    pub rez_file: PathBuf,

    /// The directory into which to extract the REZ file contents.
    pub output_directory: PathBuf,
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

fn extract_rez_entries_recursive(rez_file: &mut File, entries: &[rez::Entry], entry_base_path: &str, extract_base_path: &Path, filters: &[Regex]) {
    for entry in entries {
        let entry_path = if entry_base_path.len() > 0 {
            format!("{}/{}", entry_base_path, entry.name())
        } else {
            entry.name().into_owned()
        };

        match entry {
            rez::Entry::Directory(dir) => {
                // descend
                let mut extract_sub_path = extract_base_path.to_owned();
                extract_sub_path.push(&dir.name);

                extract_rez_entries_recursive(rez_file, &dir.entries, &entry_path, &extract_sub_path, filters);
            },
            rez::Entry::Resource(res) => {
                // check if a filter matches
                if filters.len() > 0 && filters.iter().all(|f| !f.is_match(&entry_path)) {
                    // skip
                    continue;
                }

                let mut extract_file_path = extract_base_path.to_owned();
                extract_file_path.push(format!("{}.{}", res.name, res.extension));

                println!("extracting {} as {}", entry_path, extract_file_path.display());

                std::fs::create_dir_all(extract_base_path)
                    .expect("failed to create directory tree");
                rez_file.seek(SeekFrom::Start(res.header.position.into()))
                    .expect("failed to seek within rez file");

                {
                    let mut output_file = File::create(&extract_file_path)
                        .expect("failed to open output file");
                    let mut remain_length: u64 = res.header.size.into();
                    while remain_length > 0 {
                        let mut buf = vec![0u8; 4*1024*1024];
                        let read_count: usize = remain_length.min(buf.len().try_into().unwrap()).try_into().unwrap();
                        rez_file.read_exact(&mut buf[0..read_count])
                            .expect("failed to read");
                        output_file.write_all(&buf[0..read_count])
                            .expect("failed to write");
                        remain_length -= u64::try_from(read_count).unwrap();
                    }
                    output_file.flush()
                        .expect("failed to flush output");
                }
            },
        }
    }
}

fn glob_pattern_to_regex(glob_pattern: &str) -> Regex {
    // single asterisk: sequence of anything except a slash
    const SINGLE_ASTERISK_MATCHER: &str = "[^/]+";
    // multiple asterisks: sequence of anything including a slash
    const MULTIPLE_ASTERISK_MATCHER: &str = ".+";
    // question mark: anything except a slash
    const QUESTION_MARK_MATCHER: &str = "[^/]";

    let mut asterisk_counter = 0;
    let mut regex_buf = String::with_capacity(glob_pattern.len() + 2);
    regex_buf.push('^');

    for c in glob_pattern.chars() {
        if c == '*' {
            asterisk_counter += 1;
            continue;
        } else {
            if asterisk_counter == 1 {
                regex_buf.push_str(SINGLE_ASTERISK_MATCHER);
            } else if asterisk_counter >= 2 {
                regex_buf.push_str(MULTIPLE_ASTERISK_MATCHER);
            }
            asterisk_counter = 0;
        }

        if c == '\\' {
            // normalize paths to forward slashes
            regex_buf.push('/');
            continue;
        }

        if c == '?' {
            regex_buf.push_str(QUESTION_MARK_MATCHER);
            continue;
        }

        if (c >= '0' && c <= '9') || (c >= 'A' && c <= 'Z') || (c >= 'a' && c <= 'z') || c == '<' || c == '>' {
            // spit it out verbatim
            regex_buf.push(c);
        } else {
            // escape it
            regex_buf.push('\\');
            regex_buf.push(c);
        }
    }

    // any trailing asterisks?
    if asterisk_counter == 1 {
        regex_buf.push_str(SINGLE_ASTERISK_MATCHER);
    } else if asterisk_counter > 2 {
        regex_buf.push_str(MULTIPLE_ASTERISK_MATCHER);
    }

    regex_buf.push('$');

    Regex::new(&regex_buf)
        .expect("failed to compile glob pattern to regex")
}


fn main() {
    let mode = Mode::parse();
    match mode {
        Mode::List(opts) => {
            let mut file = File::open(&opts.rez_file)
                .expect("failed to open REZ file");
            let rez_file = rez::File::try_read(&mut file)
                .expect("failed to read REZ directory");
            output_rez_entries_recursive(&rez_file.root_entries, 0);
        },
        Mode::Extract(opts) => {
            let mut file = File::open(&opts.rez_file)
                .expect("failed to open REZ file");
            let rez_file = rez::File::try_read(&mut file)
                .expect("failed to read REZ directory");
            let filters: Vec<Regex> = opts.filters
                .iter()
                .map(|s| glob_pattern_to_regex(s))
                .collect();

            extract_rez_entries_recursive(
                &mut file,
                &rez_file.root_entries,
                "",
                &opts.output_directory,
                &filters,
            );
        },
    }
}
