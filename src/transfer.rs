//! Motore di trasferimento: streaming tar/cat/dd su adb + coda di job per la TUI.
//!
//! Principio: niente MTP e niente `adb pull` per-directory; si usano pipe binarie
//! (`exec-out` / `exec-in`) con tar sul device, così un intero albero viene
//! trasferito in un solo flusso continuo, con progresso calcolato sui byte letti.

use crate::adb::Adb;
use crate::util::{dev_basename, dev_parent, sh_quote};
use anyhow::{bail, Context, Result};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Instant;

pub const MB: u64 = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Pull,
    Push,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JobKind {
    File,
    Dir,
    Sync,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
}

impl std::fmt::Display for Status {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            Status::Queued => t!("in coda"),
            Status::Running => t!("attivo"),
            Status::Done => t!("completato"),
            Status::Failed => t!("fallito"),
            Status::Cancelled => t!("annullato"),
        };
        write!(f, "{s}")
    }
}

/// Errore "operazione annullata": riconoscibile con `downcast_ref` indipendentemente dalla lingua.
#[derive(Debug)]
pub struct Cancelled;

impl std::fmt::Display for Cancelled {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", t!("annullato"))
    }
}

impl std::error::Error for Cancelled {}

/// Specifica di un lavoro per la coda della TUI.
pub struct JobSpec {
    pub id: u64,
    pub dir: Direction,
    pub kind: JobKind,
    pub src: String,
    pub dst: String,
    pub label: String,
    pub verify: bool,
    pub delete: bool,
}

impl JobSpec {
    pub fn new(
        dir: Direction,
        kind: JobKind,
        src: impl Into<String>,
        dst: impl Into<String>,
        label: impl Into<String>,
    ) -> Self {
        Self {
            id: 0,
            dir,
            kind,
            src: src.into(),
            dst: dst.into(),
            label: label.into(),
            verify: false,
            delete: false,
        }
    }
}

/// Stato corrente di un job (mutato dal worker).
#[derive(Debug)]
pub struct JobState {
    pub status: Status,
    pub bytes: u64,
    pub total: u64,
    pub msg: String,
    pub started: Option<Instant>,
    pub ended: Option<Instant>,
}

/// Vista di un job condivisa fra worker e UI.
#[derive(Debug)]
#[allow(dead_code)]
pub struct JobView {
    pub id: u64,
    pub dir: Direction,
    pub kind: JobKind,
    pub label: String,
    pub src: String,
    pub dst: String,
    pub st: Mutex<JobState>,
}

/// Interfaccia di avanzamento condivisa fra TUI e CLI.
pub trait Sink: Send + Sync {
    fn set_total(&self, t: u64);
    fn set_bytes(&self, b: u64);
    fn add_bytes(&self, n: u64);
    fn msg(&self, m: &str);
}

impl Sink for Arc<JobView> {
    fn set_total(&self, t: u64) {
        self.st.lock().unwrap().total = t;
    }
    fn set_bytes(&self, b: u64) {
        self.st.lock().unwrap().bytes = b;
    }
    fn add_bytes(&self, n: u64) {
        self.st.lock().unwrap().bytes += n;
    }
    fn msg(&self, m: &str) {
        self.st.lock().unwrap().msg = m.to_string();
    }
}

/// Wrapper che espone un `Arc<JobView>` come oggetto tratto `Sink`.
struct JobSink(Arc<JobView>);

impl Sink for JobSink {
    fn set_total(&self, t: u64) {
        self.0.set_total(t);
    }
    fn set_bytes(&self, b: u64) {
        self.0.set_bytes(b);
    }
    fn add_bytes(&self, n: u64) {
        self.0.add_bytes(n);
    }
    fn msg(&self, m: &str) {
        self.0.msg(m);
    }
}

/// Snapshot immutabile per il rendering.
#[derive(Debug, Clone)]
#[allow(dead_code)]
pub struct JobSnap {
    pub id: u64,
    pub dir: Direction,
    pub kind: JobKind,
    pub label: String,
    pub status: Status,
    pub bytes: u64,
    pub total: u64,
    pub msg: String,
    /// Secondi trascorsi dall'avvio (congelati a fine job); 0 se non ancora partito.
    pub elapsed: f64,
}

struct Inner {
    jobs: Mutex<Vec<Arc<JobView>>>,
    tx: Mutex<std::sync::mpsc::Sender<JobSpec>>,
    next_id: AtomicU64,
    flags: Mutex<HashMap<u64, Arc<AtomicBool>>>,
}

/// Coda di trasferimenti con worker dedicato (un job alla volta).
#[derive(Clone)]
pub struct Queue {
    inner: Arc<Inner>,
}

impl Queue {
    pub fn spawn(adb: Adb) -> Self {
        let (tx, rx) = std::sync::mpsc::channel::<JobSpec>();
        let inner = Arc::new(Inner {
            jobs: Mutex::new(Vec::new()),
            tx: Mutex::new(tx),
            next_id: AtomicU64::new(1),
            flags: Mutex::new(HashMap::new()),
        });
        let i2 = inner.clone();
        let _ = thread::Builder::new()
            .name("abt-worker".into())
            .spawn(move || worker(adb, rx, i2));
        Self { inner }
    }

    /// Accoda un job; restituisce l'id assegnato.
    pub fn submit(&self, mut spec: JobSpec) -> u64 {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        spec.id = id;
        let view = Arc::new(JobView {
            id,
            dir: spec.dir,
            kind: spec.kind,
            label: spec.label.clone(),
            src: spec.src.clone(),
            dst: spec.dst.clone(),
            st: Mutex::new(JobState {
                status: Status::Queued,
                bytes: 0,
                total: 0,
                msg: String::new(),
                started: None,
                ended: None,
            }),
        });
        self.inner
            .flags
            .lock()
            .unwrap()
            .insert(id, Arc::new(AtomicBool::new(false)));
        self.inner.jobs.lock().unwrap().push(view);
        let _ = self.inner.tx.lock().unwrap().send(spec);
        id
    }

    pub fn snapshot(&self) -> Vec<JobSnap> {
        self.inner
            .jobs
            .lock()
            .unwrap()
            .iter()
            .map(|v| {
                let st = v.st.lock().unwrap();
                JobSnap {
                    id: v.id,
                    dir: v.dir,
                    kind: v.kind,
                    label: v.label.clone(),
                    status: st.status,
                    bytes: st.bytes,
                    total: st.total,
                    msg: st.msg.clone(),
                    elapsed: st
                        .started
                        .map(|s| st.ended.unwrap_or_else(Instant::now).duration_since(s).as_secs_f64())
                        .unwrap_or(0.0),
                }
            })
            .collect()
    }

    /// Richiede l'annullamento: il worker termina i processi coinvolti.
    pub fn cancel(&self, id: u64) {
        if let Some(f) = self.inner.flags.lock().unwrap().get(&id) {
            f.store(true, Ordering::Relaxed);
            if let Some(v) = self
                .inner
                .jobs
                .lock()
                .unwrap()
                .iter()
                .find(|v| v.id == id)
            {
                let mut st = v.st.lock().unwrap();
                if st.status == Status::Queued {
                    st.status = Status::Cancelled;
                }
            }
        }
    }

    /// Annulla tutti i job ancora in coda (non quello in esecuzione); restituisce quanti.
    pub fn cancel_queued(&self) -> usize {
        let mut n = 0;
        for v in self.inner.jobs.lock().unwrap().iter() {
            let mut st = v.st.lock().unwrap();
            if st.status == Status::Queued {
                st.status = Status::Cancelled;
                n += 1;
                if let Some(f) = self.inner.flags.lock().unwrap().get(&v.id) {
                    f.store(true, Ordering::Relaxed);
                }
            }
        }
        n
    }

    /// Id del job attualmente in esecuzione, se presente.
    pub fn running_id(&self) -> Option<u64> {
        self.inner
            .jobs
            .lock()
            .unwrap()
            .iter()
            .find(|v| v.st.lock().unwrap().status == Status::Running)
            .map(|v| v.id)
    }
}

fn short_err(e: &anyhow::Error) -> String {
    let s = e.to_string();
    let first = s.lines().next().unwrap_or_else(|| t!("errore"));
    first.chars().take(120).collect()
}

fn worker(adb: Adb, rx: Receiver<JobSpec>, inner: Arc<Inner>) {
    for spec in rx {
        let view = {
            let jobs = inner.jobs.lock().unwrap();
            jobs.iter().find(|v| v.id == spec.id).cloned()
        };
        let Some(view) = view else { continue };
        {
            let mut st = view.st.lock().unwrap();
            if st.status == Status::Cancelled {
                continue;
            }
            st.status = Status::Running;
            st.started = Some(Instant::now());
        }
        let flag = inner
            .flags
            .lock()
            .unwrap()
            .get(&spec.id)
            .cloned()
            .unwrap_or_else(|| Arc::new(AtomicBool::new(false)));
        let sink: Arc<dyn Sink> = Arc::new(JobSink(view.clone()));
        let res = run_spec(&adb, &spec, &flag, sink.as_ref());
        let mut st = view.st.lock().unwrap();
        st.ended = Some(Instant::now());
        match &res {
            Ok(()) => {
                st.status = Status::Done;
                if st.total > 0 {
                    st.bytes = st.total;
                }
            }
            Err(e) if e.downcast_ref::<Cancelled>().is_some() => st.status = Status::Cancelled,
            Err(e) => {
                st.msg = short_err(e);
                st.status = Status::Failed;
            }
        }
        inner.flags.lock().unwrap().remove(&spec.id);
    }
}

/// Esegue una specifica di job (usata dal worker della TUI).
pub fn run_spec(adb: &Adb, spec: &JobSpec, flag: &AtomicBool, sink: &dyn Sink) -> Result<()> {
    if flag.load(Ordering::Relaxed) {
        bail!(Cancelled);
    }
    match (spec.dir, spec.kind) {
        (Direction::Pull, JobKind::File) => {
            pull_file(adb, &spec.src, &spec.dst, spec.verify, flag, sink)
        }
        (Direction::Pull, JobKind::Dir) => pull_dir(adb, &spec.src, &spec.dst, flag, sink),
        (Direction::Pull, JobKind::Sync) => sync_mirror(adb, &spec.src, &spec.dst, spec.delete, flag, sink).map(|_| ()),
        (Direction::Push, JobKind::File) => push_file(adb, &spec.src, &spec.dst, flag, sink),
        (Direction::Push, JobKind::Dir) => push_dir(adb, &spec.src, &spec.dst, flag, sink),
        _ => bail!(t!("combinazione non supportata")),
    }
}

/// Dimensione stimata sul device con `du -sk`; 0 se non determinabile.
pub fn device_size_or_zero(adb: &Adb, path: &str) -> u64 {
    let parse = |out: String| {
        out.split_whitespace()
            .next()
            .and_then(|t| t.parse::<u64>().ok())
    };
    // byte reali (apparent size); fallback su KB occupati se `-b` non c'è
    if let Some(b) = adb
        .shell(&format!("toybox du -sb {}", sh_quote(path)))
        .ok()
        .and_then(parse)
    {
        return b;
    }
    adb.shell(&format!("toybox du -sk {}", sh_quote(path)))
        .ok()
        .and_then(parse)
        .unwrap_or(0)
        * 1024
}

/// Somma ricorsiva delle dimensioni dei file locali.
pub fn local_tree_size(p: &Path) -> u64 {
    let mut sum = 0;
    if let Ok(md) = fs::metadata(p) {
        if md.is_file() {
            return md.len();
        }
        if let Ok(rd) = fs::read_dir(p) {
            for e in rd.flatten() {
                sum += local_tree_size(&e.path());
            }
        }
    }
    sum
}

/// Copia un flusso in un altro, con controllo di annullamento e callback per i byte.
fn pipe<R: Read, W: Write>(
    r: &mut R,
    w: &mut W,
    flag: &AtomicBool,
    on_chunk: &mut dyn FnMut(u64),
) -> Result<()> {
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        if flag.load(Ordering::Relaxed) {
            bail!(Cancelled);
        }
        let n = r.read(&mut buf)?;
        if n == 0 {
            return Ok(());
        }
        w.write_all(&buf[..n])?;
        on_chunk(n as u64);
    }
}

/// Pull di un singolo file con resume su `.part` (dd skip di MB interi).
pub fn pull_file(
    adb: &Adb,
    src: &str,
    dst: &str,
    verify: bool,
    flag: &AtomicBool,
    sink: &dyn Sink,
) -> Result<()> {
    let total = device_size_or_zero(adb, src);
    sink.set_total(total);
    if let Some(par) = Path::new(dst).parent() {
        fs::create_dir_all(par).with_context(|| t!("creazione di {path}", path = par.display()))?;
    }
    if let Ok(md) = fs::metadata(dst) {
        if md.is_dir() {
            bail!(t!("la destinazione {dst} esiste già ed è una directory", dst = dst));
        }
    }
    let part = format!("{dst}.part");
    let existing = fs::metadata(&part).map(|m| m.len()).unwrap_or(0);
    let skip = existing / MB * MB;
    sink.set_bytes(skip);
    let mut f = OpenOptions::new()
        .create(true)
        .write(true)
        .open(&part)
        .with_context(|| t!("apertura di {path}", path = part))?;
    f.set_len(skip)?;
    f.seek(std::io::SeekFrom::Start(skip))?;
    let cmd = format!(
        "toybox dd if={} bs={} skip={} 2>/dev/null",
        sh_quote(src),
        MB,
        skip / MB
    );
    let mut child = adb.exec_out(&cmd)?;
    let mut out = child.stdout.take().context(t!("stdout adb assente"))?;
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        if flag.load(Ordering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            bail!(Cancelled);
        }
        let n = out.read(&mut buf)?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n])?;
        sink.add_bytes(n as u64);
    }
    let st = child.wait()?;
    if flag.load(Ordering::Relaxed) {
        bail!(Cancelled);
    }
    if !st.success() {
        bail!(t!("trasferimento adb interrotto (exit status: {st}) — il file .part resta per il resume", st = st));
    }
    drop(f);
    fs::rename(&part, dst).with_context(|| t!("rinomina {from} -> {to}", from = part, to = dst))?;
    if verify {
        verify_md5(adb, src, dst)?;
    }
    Ok(())
}

/// MD5 di un file locale (nativo, senza dipendere da `md5sum`).
fn local_md5(path: &str) -> Result<String> {
    use md5::{Digest, Md5};
    let mut f = fs::File::open(path).with_context(|| t!("apertura di {path}", path = path))?;
    let mut h = Md5::new();
    let mut buf = vec![0u8; 256 * 1024];
    loop {
        let n = f.read(&mut buf)?;
        if n == 0 {
            break;
        }
        h.update(&buf[..n]);
    }
    Ok(h.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// Verifica MD5 dopo il trasferimento di un singolo file.
fn verify_md5(adb: &Adb, src: &str, dst: &str) -> Result<()> {
    let rem = adb.shell(&format!("toybox md5sum {}", sh_quote(src)))?;
    let rhash = rem.split_whitespace().next().unwrap_or("").to_lowercase();
    let lhash = local_md5(dst).unwrap_or_default();
    if rhash.is_empty() || lhash.is_empty() {
        bail!(t!("verifica MD5 non disponibile"));
    }
    if rhash != lhash {
        bail!(t!(
            "MD5 diverso dopo il trasferimento (device {dev}… vs locale {loc}…)",
            dev = &rhash[..8.min(rhash.len())],
            loc = &lhash[..8.min(lhash.len())]
        ));
    }
    Ok(())
}

/// Pull di una directory: `toybox tar` sul device -> `tar -xf` locale.
pub fn pull_dir(adb: &Adb, src: &str, dst: &str, flag: &AtomicBool, sink: &dyn Sink) -> Result<()> {
    let total = device_size_or_zero(adb, src);
    sink.set_total(total);
    fs::create_dir_all(dst).with_context(|| t!("creazione di {path}", path = dst))?;
    let parent = dev_parent(src);
    let name = dev_basename(src);
    let mut c1 = adb.exec_out(&format!(
        "toybox tar -cf - -C {} {}",
        sh_quote(&parent),
        sh_quote(name)
    ))?;
    let mut c2 = Command::new("tar")
        .arg("-xf")
        .arg("-")
        .arg("-C")
        .arg(dst)
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .context(t!("avvio tar locale"))?;
    let mut out1 = c1.stdout.take().context(t!("stdout adb assente"))?;
    let mut in2 = c2.stdin.take().context(t!("stdin tar locale"))?;
    let res = pipe(&mut out1, &mut in2, flag, &mut |n| sink.add_bytes(n));
    if let Err(e) = res {
        let _ = c1.kill();
        let _ = c2.kill();
        let _ = c1.wait();
        let _ = c2.wait();
        return Err(e);
    }
    drop(in2);
    let s1 = c1.wait()?;
    let o2 = c2.wait_with_output()?;
    if flag.load(Ordering::Relaxed) {
        bail!(Cancelled);
    }
    if !s1.success() {
        bail!(t!("tar sul device fallito (exit {s1}) — possibili permessi negati (es. Android/data)", s1 = s1));
    }
    if !o2.status.success() {
        let e = String::from_utf8_lossy(&o2.stderr);
        bail!(t!(
            "estrazione locale fallita: {msg}",
            msg = e.lines().last().unwrap_or("tar error")
        ));
    }
    Ok(())
}

fn push_via_tar(adb: &Adb, src: &Path, dst_dir: &str, flag: &AtomicBool, sink: &dyn Sink) -> Result<()> {
    adb.shell(&format!("toybox mkdir -p {}", sh_quote(dst_dir)))?;
    let parent = src.parent().unwrap_or(Path::new("."));
    let name = src
        .file_name()
        .context(t!("nome sorgente"))?
        .to_string_lossy()
        .into_owned();
    let mut ctar = Command::new("tar")
        .arg("-cf")
        .arg("-")
        .arg("-C")
        .arg(parent)
        .arg(&name)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context(t!("avvio tar locale"))?;
    let mut cadb = adb.exec_in(&format!("toybox tar -xf - -C {}", sh_quote(dst_dir)))?;
    let mut out = ctar.stdout.take().context(t!("stdout tar locale"))?;
    let mut ain = cadb.stdin.take().context(t!("stdin adb assente"))?;
    let res = pipe(&mut out, &mut ain, flag, &mut |n| sink.add_bytes(n));
    if let Err(e) = res {
        let _ = ctar.kill();
        let _ = cadb.kill();
        let _ = ctar.wait();
        let _ = cadb.wait();
        return Err(e);
    }
    drop(ain);
    let s1 = ctar.wait()?;
    let o2 = cadb.wait_with_output()?;
    if flag.load(Ordering::Relaxed) {
        bail!(Cancelled);
    }
    if !s1.success() {
        bail!(t!("tar locale fallito"));
    }
    if !o2.status.success() {
        let e = String::from_utf8_lossy(&o2.stderr);
        bail!(t!(
            "estrazione sul device fallita: {msg}",
            msg = e.lines().last().unwrap_or("error")
        ));
    }
    Ok(())
}

/// Push di un file: viene piazzato dentro `dst_dir` (creata se manca).
pub fn push_file(adb: &Adb, src: &str, dst_dir: &str, flag: &AtomicBool, sink: &dyn Sink) -> Result<()> {
    let md = fs::metadata(src).with_context(|| t!("sorgente {src} non accessibile", src = src))?;
    if !md.is_file() {
        bail!(t!("{src} non è un file", src = src));
    }
    sink.set_total(md.len());
    push_via_tar(adb, Path::new(src), dst_dir, flag, sink)
}

/// Push di una directory (tar locale -> exec-in tar sul device).
pub fn push_dir(adb: &Adb, src: &str, dst_dir: &str, flag: &AtomicBool, sink: &dyn Sink) -> Result<()> {
    let total = local_tree_size(Path::new(src));
    sink.set_total(total);
    push_via_tar(adb, Path::new(src), dst_dir, flag, sink)
}

#[derive(Debug, Default, Clone)]
pub struct SyncStats {
    pub files: u64,
    pub deleted: u64,
    pub bytes: u64,
}

fn walk_local(
    root: &Path,
    dir: &Path,
    out: &mut HashMap<String, (u64, i64)>,
    dirs: &mut Vec<String>,
) {
    let Ok(rd) = fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        let Ok(rel) = p.strip_prefix(root) else { continue };
        let Ok(md) = fs::metadata(&p) else { continue };
        let rels = rel.to_string_lossy().replace('\\', "/");
        if md.is_dir() {
            dirs.push(rels.clone());
            walk_local(root, &p, out, dirs);
        } else if md.is_file() {
            let mtime = md
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            out.insert(rels, (md.len(), mtime));
        }
    }
}

/// Mirror incrementale device -> locale: copia solo file mancanti/nuovi/variati.
pub fn sync_mirror(
    adb: &Adb,
    remote_root: &str,
    local_root: &str,
    delete: bool,
    flag: &AtomicBool,
    sink: &dyn Sink,
) -> Result<SyncStats> {
    sink.msg(t!("elenco file remoti…"));
    let cmd = format!(
        "toybox find -H {} -type f -print0 | toybox xargs -0 -n 64 toybox stat -c '%s|%Y|%n'",
        sh_quote(remote_root)
    );
    let raw = adb.exec_out_collect(&cmd)?;
    let prefix = format!("{}/", remote_root.trim_end_matches('/'));
    let mut remote: Vec<(String, u64, i64)> = Vec::new();
    for line in String::from_utf8_lossy(&raw).lines() {
        let line = line.trim_end_matches('\r');
        let mut it = line.splitn(3, '|');
        let (Some(s), Some(m), Some(p)) = (it.next(), it.next(), it.next()) else {
            continue;
        };
        let Some(rel) = p.trim().strip_prefix(&prefix) else { continue };
        if rel.is_empty() {
            continue;
        }
        remote.push((
            rel.to_string(),
            s.trim().parse().unwrap_or(0),
            m.trim().parse().unwrap_or(0),
        ));
    }

    sink.msg(t!("elenco file locali…"));
    let lr = Path::new(local_root);
    let mut local: HashMap<String, (u64, i64)> = HashMap::new();
    let mut local_dirs: Vec<String> = Vec::new();
    walk_local(lr, lr, &mut local, &mut local_dirs);

    let mut todo: Vec<(String, u64, i64)> = Vec::new();
    for (rel, size, mtime) in &remote {
        let need = match local.get(rel) {
            None => true,
            Some((lsz, lmt)) => lsz != size || *mtime > *lmt + 2,
        };
        if need {
            todo.push((rel.clone(), *size, *mtime));
        }
    }
    let total: u64 = todo.iter().map(|t| t.1).sum();
    sink.set_total(total);
    sink.msg(t!("sincronizzazione…"));

    let mut stats = SyncStats::default();
    let base = local_root.trim_end_matches('/').to_string();
    for (rel, _, _) in &todo {
        if flag.load(Ordering::Relaxed) {
            bail!(Cancelled);
        }
        sink.msg(rel);
        let dst = format!("{base}/{rel}");
        let src = format!("{prefix}{rel}");
        pull_file(adb, &src, &dst, false, flag, sink)?;
        stats.files += 1;
    }
    if delete {
        let rset: std::collections::HashSet<&String> = remote.iter().map(|r| &r.0).collect();
        for rel in local.keys() {
            if !rset.contains(rel) {
                let p = format!("{base}/{rel}");
                if fs::remove_file(&p).is_ok() {
                    stats.deleted += 1;
                }
            }
        }
        // prova a rimuovere le directory rimaste vuote, dal fondo verso l'alto
        local_dirs.sort_by_key(|d| std::cmp::Reverse(d.split('/').count()));
        for d in local_dirs {
            let _ = fs::remove_dir(format!("{base}/{d}"));
        }
    }
    stats.bytes = total;
    Ok(stats)
}
