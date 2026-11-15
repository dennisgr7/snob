//! Files a result or a download is written to: names that behave as files,
//! files made new and private, a path replaced only once the whole file has
//! arrived, and standard output for a reader that may leave.

use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result, anyhow};
use snob_core::model::printable;

/// How long a scratch directory, or a download's `.part`, has to be untouched
/// before a later run treats it as abandoned.
pub const ABANDONED_AFTER: std::time::Duration = std::time::Duration::from_secs(6 * 3600);

/// Builds a file name for a result nobody named, and refuses the ones that
/// would not behave as files ([`usable_stem`]).
pub fn default_path(in_dir: &Path, stem: &str, extension: &str) -> Result<PathBuf> {
    usable_stem(stem)?;
    let name = format!("{stem}.{extension}");
    if in_dir.join(&name).exists() {
        return Err(already_there(&name));
    }
    Ok(name.into())
}

/// A scratch file, removed when it is let go of: on a failure, and when the
/// download writing it is given up. Made once the file is the holder's own.
pub(crate) struct Part(pub(crate) PathBuf);

impl Drop for Part {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

fn already_there(name: &str) -> anyhow::Error {
    anyhow!("{name} already exists here. Use -o to say where the result should go")
}

/// `part`, a finished file, put into `dir` as `<stem>.<extension>`
/// ([`default_path`]) and removed from where it was, whatever the outcome.
///
/// **Never over a file that is there, one made since the look included**: two
/// runs saving the same name both pass [`default_path`], and a rename would
/// put the later file over the earlier.
pub(crate) fn place_new(part: &Path, dir: &Path, stem: &str, extension: &str) -> Result<PathBuf> {
    let placed = default_path(dir, stem, extension).and_then(|name| {
        let path = dir.join(&name);
        match link_new(part, &path).or_else(|_| claim_and_rename(part, &path)) {
            Ok(true) => Ok(path),
            Ok(false) => Err(already_there(&name.to_string_lossy())),
            Err(e) => Err(anyhow::Error::new(e)
                .context(format!("could not move into place {}", path.display()))),
        }
    });
    // Its other name after a link; nothing is left to remove after a rename.
    let _ = std::fs::remove_file(part);
    placed
}

/// `part` under the name `path` as well, unless something has it (`false`):
/// a link is made whole or not at all, and never replaces (`EEXIST` from
/// `linkat`, `ERROR_ALREADY_EXISTS` from `CreateHardLinkW`).
fn link_new(part: &Path, path: &Path) -> std::io::Result<bool> {
    match std::fs::hard_link(part, path) {
        Ok(()) => Ok(true),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => Ok(false),
        Err(e) => Err(e),
    }
}

/// [`link_new`] where the file system makes no links (FAT, exFAT, some
/// shares): the name is taken with an empty file made new, which is what
/// refuses a second writer, and `part` renamed over that file of its own.
fn claim_and_rename(part: &Path, path: &Path) -> std::io::Result<bool> {
    match create_new_private(path) {
        Ok(claim) => drop(claim),
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => return Ok(false),
        Err(e) => return Err(e),
    }
    std::fs::rename(part, path)
        .map(|()| true)
        .inspect_err(|_| drop(std::fs::remove_file(path)))
}

/// Refuses a stem that would not behave as a file name.
pub fn usable_stem(stem: &str) -> Result<()> {
    #[rustfmt::skip]
    const RESERVED: [&str; 24] = [
        "con", "prn", "aux", "nul", "conin$", "conout$",
        "com1", "com2", "com3", "com4", "com5", "com6", "com7", "com8", "com9",
        "lpt1", "lpt2", "lpt3", "lpt4", "lpt5", "lpt6", "lpt7", "lpt8", "lpt9",
    ];

    // Windows decides a device name from what comes before the first dot, so
    // `con.x.jpg` opens the console just as `con` does.
    let device = stem.split('.').next().unwrap_or(stem).to_ascii_lowercase();

    let usable = !stem.is_empty()
        && stem.len() <= 64
        && !stem.starts_with('.')
        // The hyphen is what `stories` puts between the name and the number:
        // a separator on no platform, and nothing a username can carry, so
        // it says nothing about the account.
        && stem
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '.' || c == '_' || c == '-')
        && !RESERVED.contains(&device.as_str());

    if !usable {
        // Filtered where it is quoted, not where it is checked: the test above
        // has to see the name Instagram sent, and this sentence is printed to a
        // terminal by `report::print_error`.
        return Err(anyhow!(
            "\"{}\" cannot be used as a file name here. \
             Use -o to say where the result should go",
            printable(stem)
        ));
    }
    Ok(())
}

/// `output::write_rendered` for bytes that are not a rendering.
pub fn write_bytes(bytes: &[u8], destination: Option<&Path>) -> Result<()> {
    match file_named(destination) {
        Some(path) => {
            std::fs::write(path, bytes)
                .with_context(|| format!("could not write {}", path.display()))?;
            crate::host::info(&format!("Written to {}", path.display()));
            Ok(())
        }
        None => write_stdout(bytes),
    }
}

/// Opens a file that must not already exist, readable by this account only.
pub fn create_new(path: &Path) -> Result<std::fs::File> {
    create_new_private(path).with_context(|| format!("could not create {}", path.display()))
}

fn create_new_private(path: &Path) -> std::io::Result<std::fs::File> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    // An export is a list of real people's names, which the umask would
    // usually leave world-readable on a shared machine.
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

/// A download into a path the user named with `-o`, which replaces what is
/// there only once the whole file has arrived.
pub struct Replacing {
    /// `None` once closed, which Windows wants before a rename or a removal.
    file: Option<std::fs::File>,
    /// The scratch file, until it is committed or abandoned; `None` when
    /// writing in place.
    part: Option<PathBuf>,
    path: PathBuf,
}

impl Replacing {
    pub fn open(path: &Path) -> Result<Self> {
        let not_written = |e: std::io::Error| anyhow!("could not write {}: {e}", path.display());
        let existing = std::fs::metadata(path).ok();
        if existing.as_ref().is_some_and(std::fs::Metadata::is_dir) {
            return Err(anyhow!(
                "{} is a directory; -o names the file this one download goes to",
                path.display()
            ));
        }
        if existing.as_ref().is_some_and(|meta| !meta.is_file()) {
            let file = std::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .map_err(not_written)?;
            return Ok(Self {
                file: Some(file),
                part: None,
                path: path.to_path_buf(),
            });
        }
        let target = match std::fs::symlink_metadata(path) {
            Ok(meta) if meta.file_type().is_symlink() => {
                std::fs::canonicalize(path).map_err(not_written)?
            }
            _ => path.to_path_buf(),
        };
        let name = target
            .file_name()
            .ok_or_else(|| anyhow!("{} names no file", path.display()))?
            .to_string_lossy()
            .into_owned();
        let dir = match target.parent() {
            Some(dir) if !dir.as_os_str().is_empty() => dir.to_path_buf(),
            _ => PathBuf::from("."),
        };
        let part = dir.join(format!(".{name}.{}.part", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&part)
            .map_err(not_written)?;
        // What the user had there keeps its permissions: the replacement is a
        // new file, and a private one should not come back readable by all.
        if let Some(meta) = existing {
            let _ = std::fs::set_permissions(&part, meta.permissions());
        }
        Ok(Self {
            file: Some(file),
            part: Some(part),
            path: target,
        })
    }

    /// Where the bytes go until [`Replacing::commit`].
    pub fn file(&mut self) -> &mut std::fs::File {
        self.file.as_mut().expect("open until committed or dropped")
    }

    /// Puts the finished file in place of the path.
    pub fn commit(mut self) -> Result<()> {
        let mut file = self.file.take().expect("open until committed or dropped");
        let Some(part) = self.part.take() else {
            return file
                .flush()
                .with_context(|| format!("could not finish writing {}", self.path.display()));
        };
        let synced = file.sync_all();
        drop(file);
        let finished = synced
            .with_context(|| format!("could not finish writing {}", self.path.display()))
            .and_then(|()| {
                std::fs::rename(&part, &self.path)
                    .with_context(|| format!("could not move into place {}", self.path.display()))
            });
        if finished.is_err() {
            let _ = std::fs::remove_file(&part);
        }
        finished
    }
}

impl Drop for Replacing {
    /// Not committed: the path is left as it was.
    fn drop(&mut self) {
        drop(self.file.take());
        if let Some(part) = self.part.take() {
            let _ = std::fs::remove_file(part);
        }
    }
}

/// Whether `-o` named standard output: `-o -`.
pub fn is_stdout(path: &Path) -> bool {
    path.as_os_str() == "-"
}

/// The file a destination names, or `None` for standard output: no `-o`, or `-o
/// -`.
pub fn file_named(destination: Option<&Path>) -> Option<&Path> {
    destination.filter(|path| !is_stdout(path))
}

/// The checks before a downloaded file goes to standard output: exactly one
/// file, and a reader that is not a terminal, where the bytes of a JPEG or an
/// MP4 are noise that can leave it in a state somebody has to reset.
pub fn one_file_to_stdout(files: usize) -> Result<()> {
    if files != 1 {
        return Err(several_to_stdout(files));
    }
    no_terminal_for_a_file()
}

/// [`one_file_to_stdout`] as far as the command line answers it, before a
/// single request: whether standard output is a terminal, and whether the
/// numbers given (`-d 1,3`) already name more than one file.
pub fn stdout_download_ahead(destination: Option<&Path>, at_least: Option<usize>) -> Result<()> {
    if !destination.is_some_and(is_stdout) {
        return Ok(());
    }
    if let Some(files) = at_least
        && files > 1
    {
        return Err(several_to_stdout(files));
    }
    no_terminal_for_a_file()
}

fn several_to_stdout(files: usize) -> anyhow::Error {
    anyhow!(
        "-o - writes one file to standard output, and this is {files}; name a \
         directory with -o for several"
    )
}

fn no_terminal_for_a_file() -> Result<()> {
    use std::io::IsTerminal;
    if std::io::stdout().is_terminal() {
        return Err(anyhow!(
            "-o - writes the file itself to standard output, which is a terminal here; \
             redirect it to a file or pipe it into another program"
        ));
    }
    Ok(())
}

/// Standard output as the sink of a streamed download, which a reader that left
/// ends quietly, as [`write_stdout`] treats one.
#[derive(Default)]
pub struct StdoutSink {
    closed: bool,
}

impl StdoutSink {
    /// What the download came to: a reader that left is not a failure, and
    /// anything else that went wrong is, flush included.
    pub fn finish<T, E: Into<anyhow::Error>>(mut self, result: Result<T, E>) -> Result<()> {
        if self.closed {
            return Ok(());
        }
        result.map_err(Into::into)?;
        match std::io::Write::flush(&mut self) {
            Ok(()) => Ok(()),
            Err(_) if self.closed => Ok(()),
            Err(e) => Err(anyhow::Error::new(e).context("could not write the file")),
        }
    }
}

impl std::io::Write for StdoutSink {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        let written = std::io::stdout().write(buf);
        self.note(written)
    }

    fn flush(&mut self) -> std::io::Result<()> {
        let flushed = std::io::stdout().flush();
        self.note(flushed)
    }
}

impl StdoutSink {
    /// Remembers a reader that left, and hands the answer on: the download
    /// stops at the error, and [`Self::finish`] reads why.
    fn note<T>(&mut self, result: std::io::Result<T>) -> std::io::Result<T> {
        if let Err(e) = &result
            && e.kind() == std::io::ErrorKind::BrokenPipe
        {
            self.closed = true;
        }
        result
    }
}

/// The result, on standard output, for a reader that may have left.
fn write_stdout(bytes: &[u8]) -> Result<()> {
    let stdout = std::io::stdout();
    let mut locked = stdout.lock();
    for step in [locked.write_all(bytes), locked.flush()] {
        match step {
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => {}
            other => other.context("could not write the result")?,
        }
    }
    Ok(())
}

/// A line of the result, on standard output -- `println!` that survives the
/// reader leaving.
pub fn say_line(line: std::fmt::Arguments<'_>) {
    let stdout = std::io::stdout();
    let mut locked = stdout.lock();
    for step in [locked.write_fmt(line), locked.write_all(b"\n")] {
        match step {
            Err(e) if e.kind() == std::io::ErrorKind::BrokenPipe => return,
            // Anything else on standard output is the terminal going away,
            // and there is nobody left to tell.
            Err(_) => return,
            Ok(()) => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `snob pfp nul` must not report success on Windows while the picture
    /// goes to the null device, and the name comes from a server rather than
    /// from anything the user typed.
    #[test]
    fn a_name_that_cannot_be_a_file_is_refused_rather_than_written() {
        // An empty directory of its own, so nothing here depends on what
        // happens to be next to the test binary or on where the suite was
        // started from.
        let dir = tempfile::tempdir().unwrap();
        let here = dir.path();

        // What `stories` and `pfp` actually invent has to pass, or the refusal
        // above is the only outcome the command can have.
        for good in ["someone-1", "some.one-12", "a_b.c", "x"] {
            assert!(
                default_path(here, good, "jpg").is_ok(),
                "{good:?} is a name this tool invents"
            );
        }

        for bad in [
            "nul",
            "CON",
            "aux",
            "com1",
            "../escape",
            "a/b",
            "",
            ".hidden",
            // Windows reads the device name up to the first dot, so these are
            // the console too, extension or no extension.
            "con.x",
            "NUL.a",
            "com3",
            "lpt9",
            "conout$",
            &"x".repeat(200),
        ] {
            assert!(
                default_path(here, bad, "jpg").is_err(),
                "{bad:?} should not become a file name"
            );
        }
        for good in ["someone", "some.one", "some_one", "user123"] {
            assert!(
                default_path(here, good, "jpg").is_ok(),
                "{good:?} should be fine"
            );
        }
    }

    /// The refusal every unusable name arrives at must not carry the name
    /// through unfiltered.
    ///
    /// A control character is one of the things that makes a name unusable
    /// here, so this refusal is where such a name always ends up — and it is
    /// printed to a terminal. `snob pfp` takes the stem from what Instagram
    /// sent, which is the side of the boundary nothing on this machine chose.
    #[test]
    fn the_refusal_does_not_print_the_name_it_is_refusing() {
        let error = default_path(Path::new("."), "gh\u{1b}[2K\u{1b}[A", "jpg")
            .unwrap_err()
            .to_string();
        assert!(!error.contains('\u{1b}'), "{error:?}");
        assert!(error.contains("gh[2K[A"), "{error:?}");
    }

    /// A finished download lands under its name, whole, and its scratch
    /// file is gone.
    #[test]
    fn a_finished_file_is_put_under_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("someone-1.7.part");
        std::fs::write(&part, b"the picture").unwrap();

        let placed = place_new(&part, dir.path(), "someone-1", "jpg").unwrap();
        assert_eq!(placed, dir.path().join("someone-1.jpg"));
        assert_eq!(std::fs::read(&placed).unwrap(), b"the picture");
        assert!(!part.exists());
    }

    /// A file that took the name after the look is not written over, by the
    /// link or by the way taken where no link can be made: the name was
    /// looked at, and then a rename replaced whatever had come since.
    #[test]
    fn a_name_taken_since_the_look_is_never_written_over() {
        type Placing = fn(&Path, &Path) -> std::io::Result<bool>;
        for (how, place) in [
            ("a link", link_new as Placing),
            ("a claim", claim_and_rename as Placing),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let part = dir.path().join("someone-1.7.part");
            let path = dir.path().join("someone-1.jpg");
            std::fs::write(&part, b"ours").unwrap();

            std::fs::write(&path, b"theirs, made since").unwrap();
            assert!(!place(&part, &path).unwrap(), "{how}");
            assert_eq!(std::fs::read(&path).unwrap(), b"theirs, made since");
            assert_eq!(std::fs::read(&part).unwrap(), b"ours", "{how}");

            std::fs::remove_file(&path).unwrap();
            assert!(place(&part, &path).unwrap(), "{how}");
            assert_eq!(std::fs::read(&path).unwrap(), b"ours", "{how}");
        }
    }

    /// Several runs that finish the same name at once leave one whole file,
    /// the first one's, and every other is told the name is taken.
    #[test]
    fn of_several_writers_of_one_name_one_wins_and_none_is_written_over() {
        const WRITERS: usize = 8;
        let dir = tempfile::tempdir().unwrap();
        let start = std::sync::Barrier::new(WRITERS);
        let outcomes: Vec<(usize, Result<PathBuf>)> = std::thread::scope(|scope| {
            let writers: Vec<_> = (0..WRITERS)
                .map(|writer| {
                    let (dir, start) = (dir.path(), &start);
                    scope.spawn(move || {
                        let part = dir.join(format!("someone-1.{writer}.part"));
                        std::fs::write(&part, format!("writer {writer}")).unwrap();
                        start.wait();
                        (writer, place_new(&part, dir, "someone-1", "jpg"))
                    })
                })
                .collect();
            writers.into_iter().map(|w| w.join().unwrap()).collect()
        });

        let won: Vec<usize> = outcomes
            .iter()
            .filter_map(|(writer, placed)| placed.is_ok().then_some(*writer))
            .collect();
        assert_eq!(won.len(), 1, "{outcomes:?}");
        for (_, placed) in outcomes.iter().filter(|(_, placed)| placed.is_err()) {
            let error = placed.as_ref().unwrap_err().to_string();
            assert!(error.contains("already exists"), "{error}");
        }
        assert_eq!(
            std::fs::read_to_string(dir.path().join("someone-1.jpg")).unwrap(),
            format!("writer {}", won[0])
        );
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        assert_eq!(left.len(), 1, "scratch files were left: {left:?}");
    }

    /// Nothing here moves the process's working directory, which `cargo test`
    /// shares between the threads of one binary.
    #[test]
    fn an_existing_file_is_not_overwritten_behind_the_users_back() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(
            default_path(dir.path(), "someone", "jpg").unwrap(),
            Path::new("someone.jpg")
        );

        std::fs::write(dir.path().join("someone.jpg"), b"something already here").unwrap();
        let error = default_path(dir.path(), "someone", "jpg")
            .unwrap_err()
            .to_string();
        assert!(error.contains("already exists"), "{error}");
    }
}
