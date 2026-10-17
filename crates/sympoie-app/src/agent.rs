use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::time::{Duration, Instant, SystemTime};

use sympoie::project::Project;
use sympoie::project_json::{MANIFEST, load_project_folder, save_project_folder};

const POLL: Duration = Duration::from_millis(250);
const QUIET: Duration = Duration::from_millis(300);

pub enum Event {
    Saved {
        token: u64,
        result: Result<(), SaveError>,
    },
    Changed {
        generation: u64,
        result: Box<Result<Project, String>>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SaveError {
    Stale,
    Failed(String),
}

impl std::fmt::Display for SaveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stale => {
                f.write_str("the project changed on disk; review those changes before saving")
            }
            Self::Failed(error) => f.write_str(error),
        }
    }
}

enum Command {
    Save {
        project: Box<Project>,
        token: u64,
        base: u64,
        reply: Option<Sender<Result<(), SaveError>>>,
    },
}

pub struct AgentLink {
    folder: PathBuf,
    commands: Sender<Command>,
    events: Receiver<Event>,
}

impl AgentLink {
    pub fn start(folder: PathBuf) -> Self {
        let (commands, command_receiver) = mpsc::channel();
        let (event_sender, events) = mpsc::channel();
        let watched = folder.clone();
        let baseline = scan(&watched, &Snapshot::default());
        std::thread::Builder::new()
            .name("sympoie-agent-link".into())
            .spawn(move || run(&watched, baseline, &command_receiver, &event_sender))
            .expect("start the agent mode worker");
        Self {
            folder,
            commands,
            events,
        }
    }

    pub fn folder(&self) -> &Path {
        &self.folder
    }

    pub fn save(&self, project: Project, token: u64, base: u64) {
        let _ = self.commands.send(Command::Save {
            project: Box::new(project),
            token,
            base,
            reply: None,
        });
    }

    pub fn save_now(&self, project: Project, base: u64) -> Result<(), SaveError> {
        let stopped = || SaveError::Failed("the agent mode worker stopped".to_owned());
        let (reply, result) = mpsc::channel();
        self.commands
            .send(Command::Save {
                project: Box::new(project),
                token: 0,
                base,
                reply: Some(reply),
            })
            .map_err(|_| stopped())?;
        result.recv().map_err(|_| stopped())?
    }

    pub fn try_event(&self) -> Option<Event> {
        self.events.try_recv().ok()
    }
}

fn run(
    folder: &Path,
    mut baseline: Snapshot,
    commands: &Receiver<Command>,
    events: &Sender<Event>,
) {
    let mut pending: Option<(Instant, Snapshot)> = None;
    let mut generation = 0;
    loop {
        match commands.recv_timeout(POLL) {
            Ok(Command::Save {
                project,
                token,
                base,
                reply,
            }) => {
                let result =
                    if base != generation || !scan(folder, &baseline).same_content(&baseline) {
                        Err(SaveError::Stale)
                    } else {
                        let result = save_project_folder(&project, folder)
                            .map(|_| ())
                            .map_err(|error| SaveError::Failed(error.to_string()));
                        baseline = scan(folder, &baseline);
                        pending = None;
                        result
                    };
                let delivered = match reply {
                    Some(reply) => reply.send(result).is_ok(),
                    None => events.send(Event::Saved { token, result }).is_ok(),
                };
                if !delivered {
                    return;
                }
            }
            Err(RecvTimeoutError::Timeout) => {
                let current = scan(folder, &baseline);
                if current.same_content(&baseline) {
                    pending = None;
                    continue;
                }
                match &pending {
                    Some((since, seen)) if seen.same_content(&current) => {
                        if since.elapsed() < QUIET {
                            continue;
                        }
                    }
                    _ => {
                        pending = Some((Instant::now(), current));
                        continue;
                    }
                }
                pending = None;
                let loaded = load_project_folder(folder).map_err(|error| error.to_string());
                baseline = current;
                generation += 1;
                let event = Event::Changed {
                    generation,
                    result: Box::new(loaded),
                };
                if events.send(event).is_err() {
                    return;
                }
            }
            Err(RecvTimeoutError::Disconnected) => return,
        }
    }
}

#[derive(Clone, Default, PartialEq)]
struct Snapshot {
    files: BTreeMap<String, FileState>,
}

#[derive(Clone, PartialEq)]
struct FileState {
    length: u64,
    modified: Option<SystemTime>,
    hash: Option<u64>,
}

impl Snapshot {
    fn same_content(&self, other: &Self) -> bool {
        self.files.len() == other.files.len()
            && self
                .files
                .iter()
                .zip(&other.files)
                .all(|((left, a), (right, b))| left == right && a.hash == b.hash)
    }
}

fn scan(folder: &Path, previous: &Snapshot) -> Snapshot {
    let mut snapshot = Snapshot::default();
    let manifest = observe(folder, MANIFEST, previous);
    let mut tracks = Vec::new();
    if manifest.hash.is_some()
        && let Ok(text) = std::fs::read_to_string(folder.join(MANIFEST))
        && let Ok(value) = sympoie::json::parse(&text)
        && let Some(sympoie::json::Value::Array(items)) = value.get("tracks")
    {
        for item in items {
            if let sympoie::json::Value::String(path) = item
                && path.starts_with("tracks/")
                && !path.contains("..")
            {
                tracks.push(path.clone());
            }
        }
    }
    snapshot.files.insert(MANIFEST.to_owned(), manifest);
    for path in tracks {
        let state = observe(folder, &path, previous);
        snapshot.files.insert(path, state);
    }
    snapshot
}

fn observe(folder: &Path, name: &str, previous: &Snapshot) -> FileState {
    let path = folder.join(name);
    let Ok(metadata) = std::fs::metadata(&path) else {
        return FileState {
            length: 0,
            modified: None,
            hash: None,
        };
    };
    let length = metadata.len();
    let modified = metadata.modified().ok();
    if let Some(known) = previous.files.get(name)
        && known.length == length
        && known.modified == modified
        && modified.is_some()
    {
        return known.clone();
    }
    FileState {
        length,
        modified,
        hash: std::fs::read(&path).ok().map(|bytes| fnv(&bytes)),
    }
}

fn fnv(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325_u64, |hash, &byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let path =
            std::env::temp_dir().join(format!("sympoie-agent-link-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&path);
        path
    }

    fn wait(link: &AgentLink) -> Event {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            if let Some(event) = link.try_event() {
                return event;
            }
            assert!(Instant::now() < deadline, "no event from the agent link");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    #[test]
    fn own_saves_are_quiet_and_outside_edits_are_loaded_once() {
        let path = folder("edits");
        let mut project = Project::new();
        project.add_midi_track("Bass");
        save_project_folder(&project, &path).unwrap();
        let link = AgentLink::start(path.clone());

        project.set_bpm(100.0).unwrap();
        link.save(project.clone(), 7, 0);
        assert!(matches!(
            wait(&link),
            Event::Saved {
                token: 7,
                result: Ok(())
            }
        ));
        link.save_now(project.clone(), 0).unwrap();
        std::thread::sleep(POLL * 3 + QUIET);
        assert!(
            link.try_event().is_none(),
            "Sympoie's own saves are not changes"
        );

        let manifest = path.join(MANIFEST);
        let text = std::fs::read_to_string(&manifest).unwrap();
        std::fs::write(
            &manifest,
            text.replace("\"tempo\": 100.0", "\"tempo\": 90.0"),
        )
        .unwrap();
        let track = std::fs::read_dir(path.join("tracks"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|file| file.to_string_lossy().ends_with("-bass.json"))
            .unwrap();
        let text = std::fs::read_to_string(&track).unwrap();
        std::fs::write(&track, text.replace("\"Bass\"", "\"Sub\"")).unwrap();
        let Event::Changed { generation, result } = wait(&link) else {
            panic!("expected a change")
        };
        assert_eq!(generation, 1);
        let loaded = result.unwrap();
        assert_eq!(loaded.bpm(), 90.0);
        assert_eq!(loaded.tracks()[1].name(), "Sub");
        std::thread::sleep(POLL * 3 + QUIET);
        assert!(link.try_event().is_none(), "one change, one event");
        assert_eq!(link.save_now(project.clone(), 0), Err(SaveError::Stale));
        assert_eq!(load_project_folder(&path).unwrap().bpm(), 90.0);
        link.save_now(loaded, 1).unwrap();

        std::fs::write(&manifest, std::fs::read(&manifest).unwrap()).unwrap();
        std::thread::sleep(POLL * 3 + QUIET);
        assert!(link.try_event().is_none());
        let track = path.join("tracks/2-sub.json");
        std::fs::write(&track, "{ \"type\": \"midi\", }").unwrap();
        let Event::Changed { generation, result } = wait(&link) else {
            panic!("expected a change")
        };
        assert_eq!(generation, 2);
        assert!(result.unwrap_err().contains("trailing comma"));
        drop(link);
        std::fs::remove_dir_all(path).unwrap();
    }
}
