use std::{
    env, fs, io,
    str::FromStr,
    collections::HashSet,
    path::{Path, PathBuf},
    process::{Command, Stdio, exit},
    io::{Read, Result, Error, Write},
    fs::{File, write, read_to_string},
    os::unix::{fs::{MetadataExt, PermissionsExt}, process::CommandExt}
};

use which::which;
use walkdir::WalkDir;

const SHARUN_NAME: &str = env!("CARGO_PKG_NAME");

fn get_interpreter(library_path: &str) -> Result<PathBuf> {
    let mut interpreters = Vec::new();
    if let Ok(ldname) = env::var("SHARUN_LDNAME") {
        if !ldname.is_empty() {
            interpreters.push(ldname)
        }
    } else {
        #[cfg(target_arch = "x86_64")]
        interpreters.append(&mut vec![
            "ld-linux-x86-64.so.2".into(),
            "ld-musl-x86_64.so.1".into(),
            "ld-linux.so.2".into()
        ]);
        #[cfg(target_arch = "aarch64")]
        interpreters.append(&mut vec![
            "ld-linux-aarch64.so.1".into(),
            "ld-musl-aarch64.so.1".into()
        ]);
    }
    for interpreter in interpreters {
        let interpreter_path = Path::new(library_path).join(interpreter);
        if interpreter_path.exists() {
            return Ok(interpreter_path)
        }
    }
    Err(Error::last_os_error())
}

fn realpath(path: &str) -> String {
    Path::new(path).canonicalize().unwrap().to_str().unwrap().to_string()
}

fn basename(path: &str) -> String {
    let pieces: Vec<&str> = path.rsplit('/').collect();
    pieces.first().unwrap().to_string()
}

fn dirname(path: &str) -> String {
    let mut pieces: Vec<&str> = path.split('/').collect();
    if pieces.len() == 1 || path.is_empty() {
        // return ".".to_string();
    } else if !path.starts_with('/') &&
        !path.starts_with('.') &&
        !path.starts_with('~') {
            pieces.insert(0, ".");
    } else if pieces.len() == 2 && path.starts_with('/') {
        pieces.insert(0, "");
    };
    pieces.pop();
    pieces.join(&'/'.to_string())
}

fn is_file(path: &str) -> bool {
    let path = Path::new(path);
    path.is_file()
}

fn is_exe(file_path: &PathBuf) -> Result<bool> {
    let metadata = fs::metadata(file_path)?;
    Ok(metadata.is_file() && metadata.permissions().mode() & 0o111 != 0)
}

fn get_env_var(var: &str) -> String {
    env::var(var).unwrap_or("".into())
}

fn gen_library_path(library_path: &mut String, lib_path_file: &String) {
    let mut new_paths: Vec<String> = Vec::new();
    WalkDir::new(&mut *library_path)
        .into_iter()
        .filter_map(|entry| entry.ok())
        .for_each(|entry| {
            let name = entry.file_name().to_string_lossy();
            if name.ends_with(".so") || name.contains(".so.") {
                if let Some(parent) = entry.path().parent() {
                    if let Some(parent_str) = parent.to_str() {
                        if parent_str != library_path && parent.is_dir() &&
                            !new_paths.contains(&parent_str.into()) {
                            new_paths.push(parent_str.into());
                        }
                    }
                }
            }
        });
    if let Err(err) = write(lib_path_file,
        format!("+:{}", &new_paths.join(":"))
            .replace(":", "\n")
            .replace(&*library_path, "+")
    ) {
        eprintln!("Failed to write lib.path: {lib_path_file}: {err}");
        exit(1)
    } else {
        eprintln!("Write lib.path: {lib_path_file}")
    }
}

fn print_usage() {
    println!("[ {} ]

[ Usage ]: {SHARUN_NAME} [OPTIONS] [EXEC ARGS]...
    Executes binaries within the specified environment.

[ Arguments ]:
    [EXEC ARGS]...              Command line arguments for execution

[ Options ]:
    -g,  --gen-lib-path         Generate library path file
    -v,  --version              Print version
    -h,  --help                 Print help

[ Environments ]:
    SHARUN_WORKING_DIR=/path    Specifies the path to the working directory
    SHARUN_LDNAME=ld.so         Specifies the name of the interpreter
    SHARUN_DIR                  Sharun directory",
    env!("CARGO_PKG_DESCRIPTION"));
}

fn main() {
    let sharun: PathBuf = env::current_exe().unwrap();
    let mut exec_args: Vec<String> = env::args().collect();

    let mut sharun_dir = sharun.parent().unwrap().to_str().unwrap().to_string();
    let lower_dir = &format!("{sharun_dir}/../");
    if basename(&sharun_dir) == "bin" &&
       is_file(&format!("{lower_dir}{SHARUN_NAME}")) {
        sharun_dir = realpath(lower_dir)
    }

    env::set_var("SHARUN_DIR", &sharun_dir);

    let bin_dir = &format!("{sharun_dir}/bin");
    let shared_dir = &format!("{sharun_dir}/shared");
    let shared_bin = &format!("{shared_dir}/bin");
    let shared_lib = format!("{shared_dir}/lib");
    let shared_lib32 = format!("{shared_dir}/lib32");

    if exec_args.is_empty() {
        print_usage();
        return;
    }

    let command = exec_args.remove(0);
    if command == "-h" || command == "--help" {
        print_usage();
    } else if command == "-v" || command == "--version" {
        println!("v{}", env!("CARGO_PKG_VERSION"));
    } else if command == "-g" || command == "--gen-lib-path" {
        for mut library_path in [shared_lib, shared_lib32] {
            if Path::new(&library_path).exists() {
                let lib_path_file = &format!("{library_path}/lib.path");
                gen_library_path(&mut library_path, lib_path_file)
            }
        }
    } else {
        eprintln!("Unrecognized command: {}", command);
    }
}
