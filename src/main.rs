use std::{
    env, fs,
    path::{Path, PathBuf},
    process::{Command, Stdio, exit},
    io::{Read, Result, Error, Write},
    fs::{File, write, read_to_string},
    os::unix::{fs::{MetadataExt, PermissionsExt}, process::CommandExt}
};

use which::which;
use walkdir::WalkDir;
use std::ffi::CString;
use std::borrow::Cow;

const SHARUN_NAME: &str = env!("CARGO_PKG_NAME");

fn expand_env_variables(input: &str) -> Cow<'_, str> {
    shellexpand::env(input).unwrap_or_else(|err| {
        eprintln!("Failed to expand environment variable: {err}");
        Cow::Borrowed(input)
    })
}

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

fn read_dotenv(working_dir: &str) {
    let dotenv_path = Path::new(working_dir).join(".env");
    if let Ok(contents) = fs::read_to_string(&dotenv_path) {
        for line in contents.lines().filter(|line| !line.trim().is_empty() && !line.starts_with('#')) {
            if let Some((key, value)) = line.split_once('=') {
                let expanded_value = expand_env_variables(value.trim());
                env::set_var(key.trim(), expanded_value.as_ref());
            }
        }
    }
}

fn main() {
    let lib4bin = include_bytes!("../lib4bin");

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

    let arg0 = PathBuf::from(exec_args.remove(0));
    let arg0_name = arg0.file_name().unwrap();
    let arg0_dir = PathBuf::from(dirname(arg0.to_str().unwrap())).canonicalize()
        .unwrap_or_else(|_|{
            if let Ok(which_arg0) = which(arg0_name) {
                which_arg0.parent().unwrap().to_path_buf()
            } else {
                eprintln!("Failed to find ARG0 dir!");
                exit(1)
            }
    });
    let arg0_path = arg0_dir.join(arg0_name);

    let mut bin_name = if arg0_path.is_symlink() && arg0_path.canonicalize().unwrap() == sharun {
        arg0_name.to_str().unwrap().into()
    } else {
        basename(sharun.file_name().unwrap().to_str().unwrap())
    };

    if bin_name == SHARUN_NAME {
        if !exec_args.is_empty() {
            match exec_args[0].as_str() {
                "-v" | "--version" => {
                    println!("v{}", env!("CARGO_PKG_VERSION"));
                    return
                }
                "-h" | "--help" => {
                    print_usage();
                    return
                }
                "-g" | "--gen-lib-path" => {
                    for mut library_path in [shared_lib, shared_lib32] {
                        if Path::new(&library_path).exists() {
                            let lib_path_file = &format!("{library_path}/lib.path");
                            gen_library_path(&mut library_path, lib_path_file)
                        }
                    }
                    return
                }
                "l" | "lib4bin" => {
                    exec_args.remove(0);
                    let cmd = Command::new("bash")
                        .env("SHARUN", sharun)
                        .envs(env::vars())
                        .stdin(Stdio::piped())
                        .arg("-s").arg("--")
                        .args(exec_args)
                        .spawn();
                    match cmd {
                        Ok(mut bash) => {
                            bash.stdin.take().unwrap().write_all(lib4bin).unwrap_or_else(|err|{
                                eprintln!("Failed to write lib4bin to bash stdin: {err}");
                                exit(1)
                            });
                            exit(bash.wait().unwrap().code().unwrap())
                        }
                        Err(err) => {
                            eprintln!("Failed to run bash: {err}");
                            exit(1)
                        }
                    }
                }
                _ => {
                    bin_name = exec_args.remove(0);
                    let bin_path = PathBuf::from(bin_dir).join(&bin_name);
                    if is_exe(&bin_path).unwrap_or(false) &&
                        (is_hardlink(&sharun, &bin_path).unwrap_or(false) ||
                        !Path::new(&shared_bin).join(&bin_name).exists())
                    {
                        add_to_env("PATH", bin_dir);
                        let err = Command::new(&bin_path)
                            .envs(env::vars())
                            .args(exec_args)
                            .exec();
                        eprintln!("Failed to run: {}: {err}", bin_path.display());
                        exit(1)
                    }
                }
            }
        } else {
            eprintln!("Specify the executable from: '{bin_dir}'");
            if let Ok(dir) = Path::new(bin_dir).read_dir() {
                for bin in dir.flatten() {
                    if is_exe(&bin.path()).unwrap_or(false) {
                        println!("{}", bin.file_name().to_str().unwrap())
                    }
                }
            }
            exit(1)
        }
    } else if bin_name == "AppRun" {
        let appname_file = &format!("{sharun_dir}/.app");
        let mut appname: String = "".into();
        if !Path::new(appname_file).exists() {
            if let Ok(dir) = Path::new(&sharun_dir).read_dir() {
                for entry in dir.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        let name = entry.file_name();
                        let name = name.to_str().unwrap();
                        if name.ends_with(".desktop") {
                            let data = read_to_string(path).unwrap_or_else(|err|{
                                eprintln!("Failed to read desktop file: {name}: {err}");
                                exit(1)
                            });
                            appname = data.split("\n").filter_map(|string| {
                                if string.starts_with("Exec=") {
                                    Some(string.replace("Exec=", "").split_whitespace().next().unwrap_or("").into())
                                } else {None}
                            }).next().unwrap_or_else(||"".into())
                        }
                    }
                }
            }
        }

        if appname.is_empty() {
            appname = read_to_string(appname_file).unwrap_or_else(|err|{
                eprintln!("Failed to read .app file: {appname_file}: {err}");
                exit(1)
            })
        }

        if let Some(name) = appname.trim().split("\n").next() {
            appname = basename(name)
            .replace("'", "").replace("\"", "")
        } else {
            eprintln!("Failed to get app name: {appname_file}");
            exit(1)
        }
        let app = &format!("{bin_dir}/{appname}");

        add_to_env("PATH", bin_dir);
        if get_env_var("ARGV0").is_empty() {
            env::set_var("ARGV0", &arg0)
        }
        env::set_var("APPDIR", &sharun_dir);

        let err = Command::new(app)
            .envs(env::vars())
            .args(exec_args)
            .exec();
        eprintln!("Failed to run App: {app}: {err}");
        exit(1)
    }
    let bin = format!("{shared_bin}/{bin_name}");

    let is_elf32_bin = is_elf32(&bin).unwrap_or_else(|err|{
        eprintln!("Failed to check ELF class: {bin}: {err}");
        exit(1)
    });

    let library_path = if is_elf32_bin {
        shared_lib32
    } else {
        shared_lib
    };

    read_dotenv(&sharun_dir);

    let interpreter = get_interpreter(&library_path).unwrap_or_else(|_|{
        eprintln!("Interpreter not found!");
        exit(1)
    });

    let working_dir = &get_env_var("SHARUN_WORKING_DIR");
    if !working_dir.is_empty() {
        env::set_current_dir(working_dir).unwrap_or_else(|err|{
            eprintln!("Failed to change working directory: {working_dir}: {err}");
            exit(1)
        });
        env::remove_var("SHARUN_WORKING_DIR")
    }

    let envs: Vec<CString> = env::vars()
        .map(|(key, value)| CString::new(
            format!("{}={}", key, value)
        ).unwrap()).collect();

    let mut interpreter_args = vec![
        CString::new(interpreter.to_string_lossy().into_owned()).unwrap(),
        CString::new("--library-path").unwrap(),
        CString::new(library_path).unwrap(),
        CString::new(bin).unwrap()
    ];
    for arg in exec_args {
        interpreter_args.push(CString::new(arg).unwrap())
    }

    userland_execve::exec(
        interpreter.as_path(),
        &interpreter_args,
        &envs,
    )
}

fn add_to_env(key: &str, value: &str) {
    let current_value = env::var(key).unwrap_or_default();
    let new_value = format!("{}:{}", value, current_value);
    env::set_var(key, new_value);
}

fn is_hardlink(file1: &Path, file2: &Path) -> Result<bool> {
    let metadata1 = fs::metadata(file1)?;
    let metadata2 = fs::metadata(file2)?;
    Ok(metadata1.ino() == metadata2.ino() && metadata1.dev() == metadata2.dev())
}

fn is_elf32(file_path: &str) -> Result<bool> {
    let mut file = File::open(file_path)?;
    let mut buffer = [0u8; 6];
    file.read_exact(&mut buffer)?;

    // Check the ELF magic number and class
    if &buffer[0..4] == b"\x7fELF" {
        Ok(buffer[4] == 1) // ELFCLASS32
    } else {
        Ok(false)
    }
}
