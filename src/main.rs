use std::collections::HashMap;
use std::env;
use std::ffi::OsString;
use std::io;
use std::os::unix::prelude::CommandExt;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::str;
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::process::Command;

use std::io::Write;

// Entry point for the server
#[tokio::main]
async fn main() -> io::Result<()> {
    let args: Vec<String> = std::env::args().collect();
    if args.len() != 3 {
        eprintln!("Usage: rustywebserver PORT ROOT_FOLDER");
        std::process::exit(1);
    }

    let port = args[1].parse::<u16>().expect("Invalid port number");
    let root_folder = PathBuf::from(&args[2]);

    println!("Root folder: {:?}", root_folder);
    println!("Server listening on 0.0.0.0:{}", port);

    let listener = TcpListener::bind(format!("0.0.0.0:{}", port)).await?;
    loop {
        let (stream, _) = listener.accept().await?;
        let root_folder = root_folder.clone();
        tokio::spawn(async move {
            if let Err(e) = handle_request(stream, root_folder).await {
                eprintln!("Error handling request: {}", e);
            }
        });
    }
}

async fn handle_request(stream: TcpStream, root_folder: PathBuf) -> io::Result<()> {
    let mut buffer = [0; 4096];
    let mut stream = stream;

    // Read request from stream
    let n = stream.read(&mut buffer).await?;
    if n == 0 {
        return Ok(());
    }

    let request = str::from_utf8(&buffer[..n]).unwrap_or("");

    // Parse request
    let lines: Vec<&str> = request.lines().collect();
    if lines.is_empty() {
        return Ok(());
    }

    let request_line = lines[0];
    let parts: Vec<&str> = request_line.split_whitespace().collect();
    if parts.len() < 3 {
        return Ok(());
    }

    let method = parts[0];
    let file_path = root_folder.join(&parts[1][1..]); // Remove the leading '/' and join with root folder
    let http_version = parts[2];

    let forbidden_files = vec![root_folder.join("forbidden.html")];
    let forbidden_dirs = vec![root_folder.join("forbidden"), root_folder.join("secret")];

    let response = if file_path.is_dir() {
        match generate_directory_listing(&file_path).await {
            Ok(html) => {
                let status_code = "200";
                let status_text = "OK";
                stream.write_all(format!("{} {} {}\r\nContent-Type: text/html; charset=utf-8\r\nConnection: close\r\n\r\n{}", http_version, status_code, status_text, html).as_bytes()).await?;
                log_connection(method, &stream, &file_path, status_code, status_text);
                return Ok(());
            }
            Err(_) => {
                let status_code = "500";
                let status_text = "Internal Server Error";
                stream
                    .write_all(
                        format!(
                            "{} {} {}\r\nConnection: close\r\n\r\n",
                            http_version, status_code, status_text
                        )
                        .as_bytes(),
                    )
                    .await?;
                log_connection(method, &stream, &file_path, status_code, status_text);
                return Ok(());
            }
        }
    } else if file_path.exists() && file_path.is_file() {
        if forbidden_files.contains(&file_path)
            || forbidden_dirs.iter().any(|dir| file_path.starts_with(dir))
        {
            let status_code = "403";
            let status_text = "Forbidden";
            stream
                .write_all(
                    format!(
                        "{} {} {}\r\nConnection: close\r\n\r\n",
                        http_version, status_code, status_text
                    )
                    .as_bytes(),
                )
                .await?;
            log_connection(method, &stream, &file_path, status_code, status_text);
            return Ok(());
        } else if file_path.starts_with(root_folder.join("scripts")) {
            match execute_script(
                &file_path,
                method,
                file_path.to_str().unwrap(),
                &HashMap::new(),
                None,
            )
            .await
            {
                Ok(output) => {
                    let status_code = "200";
                    let status_text = "OK";
                    stream
                        .write_all(
                            format!(
                                "{} {} {}\r\nConnection: close\r\n\r\n{}",
                                http_version, status_code, status_text, output
                            )
                            .as_bytes(),
                        )
                        .await?;
                    log_connection(method, &stream, &file_path, status_code, status_text);
                    return Ok(());
                }
                Err(err) => {
                    let status_code = "500";
                    let status_text = "Internal Server Error";
                    stream
                        .write_all(
                            format!(
                                "{} {} {}\r\nConnection: close\r\n\r\n{}",
                                http_version, status_code, status_text, err
                            )
                            .as_bytes(),
                        )
                        .await?;
                    log_connection(method, &stream, &file_path, status_code, status_text);
                    return Ok(());
                }
            }
        } else {
            match read_file(&file_path).await {
                Ok(contents) => {
                    let mime_type = get_mime_type(&file_path);
                    let status_code = "200";
                    let status_text = "OK";
                    let header = format!(
                        "{} {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        http_version, status_code, status_text, mime_type, contents.len()
                    );
                    stream.write_all(header.as_bytes()).await?;
                    stream.write_all(&contents).await?;
                    log_connection(method, &stream, &file_path, status_code, status_text);
                    return Ok(());
                }
                Err(_) => {
                    let status_code = "403";
                    let status_text = "Forbidden";
                    stream
                        .write_all(
                            format!(
                                "{} {} {}\r\nConnection: close\r\n\r\n",
                                http_version, status_code, status_text
                            )
                            .as_bytes(),
                        )
                        .await?;
                    log_connection(method, &stream, &file_path, status_code, status_text);
                    return Ok(());
                }
            }
        }
    } else {
        let status_code = "404";
        let status_text = "Not Found";
        stream
            .write_all(
                format!(
                    "{} {} {}\r\nConnection: close\r\n\r\n",
                    http_version, status_code, status_text
                )
                .as_bytes(),
            )
            .await?;
        log_connection(method, &stream, &file_path, status_code, status_text);
        return Ok(());
    };
}

// Function to log connection details
fn log_connection(
    method: &str,
    stream: &TcpStream,
    file_path: &Path,
    status_code: &str,
    status_text: &str,
) {
    if let Ok(remote_addr) = stream.peer_addr() {
        println!(
            "{} {} {} -> {} ({})",
            method,
            remote_addr,
            file_path.display(),
            status_code,
            status_text
        );
        io::stdout().flush().unwrap(); // Ensure that the log is flushed to the terminal
    } else {
        println!(
            "{} unknown {} -> {} ({})",
            method,
            file_path.display(),
            status_code,
            status_text
        );
        io::stdout().flush().unwrap(); // Ensure that the log is flushed to the terminal
    }
}

// Generate directory listing
async fn generate_directory_listing(path: &Path) -> Result<String, io::Error> {
    let mut entries = fs::read_dir(path).await?;
    let mut file_names = vec![];

    while let Some(entry) = entries.next_entry().await? {
        let file_name = entry.file_name().to_string_lossy().to_string();
        file_names.push(file_name);
    }

    let mut html = String::from("<html>\n<h1>Directory Listing</h1>\n<ul>\n");
    html.push_str(r#"<li><a href="/..">..</a></li>"#);
    for file_name in file_names {
        html.push_str(&format!(
            r#"<li><a href="/{}">{}</a></li>"#,
            file_name, file_name
        ));
    }
    html.push_str("</ul>\n</html>");

    Ok(html)
}

// Execute script
async fn execute_script(
    script_path: &Path,
    method: &str,
    request_path: &str,
    headers: &HashMap<String, String>,
    body: Option<String>,
) -> Result<String, String> {
    let mut command = Command::new(script_path);
    command.stdin(Stdio::piped());
    command.env("Method", method);
    command.env("Path", request_path);

    for (key, value) in headers {
        command.env(format!("Query_{}", key), value);
    }

    let mut child = command
        .spawn()
        .map_err(|err| format!("Failed to start script: {}", err))?;

    if let Some(body) = body {
        if let Some(stdin) = child.stdin.as_mut() {
            stdin
                .write_all(body.as_bytes())
                .await
                .map_err(|err| format!("Failed to write to script stdin: {}", err))?;
        }
    }

    let output = child
        .wait_with_output()
        .await
        .map_err(|err| format!("Script execution failed: {}", err))?;

    if !output.status.success() {
        return Err(String::from_utf8_lossy(&output.stderr).to_string());
    }

    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

// Read file contents
async fn read_file(file_path: &Path) -> Result<Vec<u8>, io::Error> {
    fs::read(file_path).await
}

// Determine MIME type based on file extension
fn get_mime_type(file_path: &Path) -> &'static str {
    match file_path.extension().and_then(|s| s.to_str()) {
        Some("html") | Some("htm") => "text/html; charset=utf-8",
        Some("css") => "text/css; charset=utf-8",
        Some("js") => "text/javascript; charset=utf-8",
        Some("txt") => "text/plain; charset=utf-8",
        Some("jpeg") | Some("jpg") => "image/jpeg",
        Some("png") => "image/png",
        Some("zip") => "application/zip",
        _ => "application/octet-stream",
    }
}
