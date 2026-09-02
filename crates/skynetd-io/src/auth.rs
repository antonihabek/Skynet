//! Local access control (§39).
//!
//! > **The stop control on the on-robot console requires no authentication.**
//! >
//! > This inverts the usual rule, and it is correct. An authentication prompt between a person and
//! > a stop button is a hazard. The threat model — somebody on the robot's LAN maliciously stopping
//! > it — is a nuisance; the threat model it prevents — somebody watching a robot do something
//! > dangerous while hunting for a password — is an injury.
//!
//! | Role | May |
//! |---|---|
//! | *(unauthenticated)* | Stop. See status. |
//! | Observer | Read health, guards, plan, packages, black box index |
//! | Operator | Start and stop packages, grant and revoke capabilities, seal and download the black box, apply a staged update |
//! | Admin | Pair and unpair, install drivers, change profiles, factory reset |
//!
//! # There is no default password, at any point in the lifecycle
//!
//! > An unprovisioned robot's console offers status and stop and nothing else, so the failure mode
//! > of never provisioning it is a useless console rather than an open one.
//!
//! [`Credentials::empty`] is what an unprovisioned robot has, and every authenticated route on it
//! answers `401`. That is not a degraded mode to be fixed by a fallback — it is the specified
//! behaviour, and a fallback is precisely the thing that would make "never provisioned" mean "open".
//!
//! # What the token check is, and what it is not
//!
//! Tokens are stored as a salted sha256 and compared in constant time. That is enough for a
//! credential issued at pairing to a client we ship, over a link that is already link-local — and
//! it is *not* a password hash. Nothing here is memory-hard, so a credential must be high-entropy
//! and machine-generated rather than chosen by a person. [`Credential::issue`] is the only way to
//! make one and it takes its randomness from the caller, so a robot cannot end up with a token
//! somebody typed.

use std::collections::BTreeMap;

use skynet_json::Json;

/// What a caller is allowed to do.
///
/// Ordered, so `role >= Role::Operator` reads as "at least an operator". The ordering is the whole
/// authorisation model and putting it in the type means a route cannot accidentally compare two
/// roles for equality and let an admin be refused something an operator may do.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Role {
    /// Nobody has proved anything. Stop, and see status.
    Anonymous,
    /// Read.
    Observer,
    /// Start, stop, grant, revoke, seal.
    Operator,
    /// Pair, install drivers, change profiles, factory reset.
    Admin,
}

impl Role {
    /// The wire spelling.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Anonymous => "anonymous",
            Self::Observer => "observer",
            Self::Operator => "operator",
            Self::Admin => "admin",
        }
    }

    /// Parse a role.
    ///
    /// `None` for anything unrecognised, and the caller treats that as [`Role::Anonymous`]. A
    /// credentials file naming a role this edition does not implement must not grant the nearest
    /// thing to it.
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "anonymous" => Some(Self::Anonymous),
            "observer" => Some(Self::Observer),
            "operator" => Some(Self::Operator),
            "admin" => Some(Self::Admin),
            _ => None,
        }
    }
}

/// One issued credential.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Credential {
    /// A name for the audit log — which laptop, which operator.
    pub label: String,
    /// What it may do.
    pub role: Role,
    /// Random per credential, so two identical tokens do not produce identical stored hashes.
    pub salt: String,
    /// sha256(salt ‖ token), hex.
    pub hash: String,
}

impl Credential {
    /// Issue one from caller-supplied randomness.
    ///
    /// The token is returned once and never stored. `salt` and `token` are both hex strings the
    /// caller generated; nothing here has an opinion about where randomness comes from, because a
    /// randomness source is a platform question and a wrong answer to it is silent.
    pub fn issue(label: &str, role: Role, salt: &str, token: &str) -> Self {
        Self {
            label: label.to_string(),
            role,
            salt: salt.to_string(),
            hash: hash_token(salt, token),
        }
    }

    /// Whether a presented token is this one.
    pub fn matches(&self, token: &str) -> bool {
        constant_time_eq(&self.hash, &hash_token(&self.salt, token))
    }
}

/// sha256 of the salt and the token, hex.
pub fn hash_token(salt: &str, token: &str) -> String {
    let mut hasher = skynet_sha256::Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(b"\0");
    hasher.update(token.as_bytes());
    let hex = skynet_sha256::hex_of(&hasher.finish());
    String::from_utf8_lossy(&hex).into_owned()
}

/// Compare two hex digests without an early exit.
///
/// Both operands are hashes rather than secrets, so this is defence in depth rather than the thing
/// standing between an attacker and the robot. It costs four lines.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut difference = 0u8;
    for (x, y) in a.bytes().zip(b.bytes()) {
        difference |= x ^ y;
    }
    difference == 0
}

/// Every credential the robot holds.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Credentials {
    entries: Vec<Credential>,
}

impl Credentials {
    /// An unprovisioned robot. Status and stop, and nothing else.
    pub fn empty() -> Self {
        Self::default()
    }

    /// Whether anything has been issued.
    pub fn is_provisioned(&self) -> bool {
        !self.entries.is_empty()
    }

    /// Add a credential.
    pub fn add(&mut self, credential: Credential) {
        self.entries.push(credential);
    }

    /// Every credential, for reporting. Hashes are included; tokens do not exist to include.
    pub fn entries(&self) -> &[Credential] {
        &self.entries
    }

    /// What role a presented token carries.
    ///
    /// Every credential is checked even after one matches. An early return would make the time this
    /// takes depend on where in the file the matching credential sits, which is a small leak and a
    /// free one to close.
    pub fn role_of(&self, token: &str) -> Role {
        let mut role = Role::Anonymous;
        for credential in &self.entries {
            if credential.matches(token) && credential.role > role {
                role = credential.role;
            }
        }
        role
    }

    /// The operator's own label for whatever presented this header.
    ///
    /// `None` when nothing was presented or nothing matched. Here because §38.3 asks a change to
    /// carry its provenance and the audit log's `actor` field wants *"a credential label, a person,
    /// a provisioning run"* — and a log that recorded every pairing as `admin` would tell an
    /// investigator which role did it and never which person.
    pub fn label_of_header(&self, header: Option<&str>) -> Option<&str> {
        let token = header?.strip_prefix("Bearer ")?.trim();
        if token.is_empty() {
            return None;
        }
        self.entries()
            .iter()
            .find(|credential| credential.matches(token))
            .map(|credential| credential.label.as_str())
    }

    /// The role a request carries, from its `Authorization` header.
    ///
    /// Anything that is not a well-formed bearer token is [`Role::Anonymous`] — which still gets
    /// status and stop, and nothing else. A malformed credential is not an error to report, it is
    /// simply not a credential.
    pub fn role_of_header(&self, header: Option<&str>) -> Role {
        let Some(header) = header else {
            return Role::Anonymous;
        };
        let Some(token) = header.strip_prefix("Bearer ").map(str::trim) else {
            return Role::Anonymous;
        };
        if token.is_empty() {
            return Role::Anonymous;
        }
        self.role_of(token)
    }

    /// Read the credentials file.
    pub fn from_json(document: &Json) -> Self {
        let mut credentials = Self::default();
        for entry in document.arr_or_empty("credentials") {
            let text = |key: &str| {
                entry
                    .get_present(key)
                    .and_then(Json::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            let Some(role) = Role::parse(&text("role")) else {
                continue;
            };
            let (salt, hash) = (text("salt"), text("hash"));
            if salt.is_empty() || hash.is_empty() {
                continue;
            }
            credentials.add(Credential {
                label: text("label"),
                role,
                salt,
                hash,
            });
        }
        credentials
    }

    /// Write the credentials file.
    pub fn to_json(&self) -> Json {
        let mut list = Json::arr();
        for credential in &self.entries {
            let mut item = Json::obj();
            item.set("label", Json::from(credential.label.as_str()))
                .set("role", Json::from(credential.role.as_str()))
                .set("salt", Json::from(credential.salt.as_str()))
                .set("hash", Json::from(credential.hash.as_str()));
            list.push(item);
        }
        let mut document = Json::obj();
        document.set("credentials", list);
        document
    }
}

/// Read `length` bytes of randomness and return them as hex.
///
/// From `/dev/urandom`, and from nowhere else. §39.2 says local credentials are issued at pairing
/// and stored hashed, and a credential is only as good as the entropy behind it — so this returns
/// an error rather than falling back to a clock, a process id, or anything else that would produce
/// a token somebody could guess. A robot that cannot generate a credential must not issue a weak
/// one; it must say it cannot.
#[cfg(unix)]
pub fn random_hex(length: usize) -> std::io::Result<String> {
    use std::io::Read;
    let mut file = std::fs::File::open("/dev/urandom")?;
    let mut bytes = vec![0u8; length];
    file.read_exact(&mut bytes)?;
    let mut hex = String::with_capacity(length * 2);
    for byte in bytes {
        hex.push_str(&format!("{byte:02x}"));
    }
    Ok(hex)
}

/// Randomness is the operating system's to give, and this one has no `/dev/urandom`.
///
/// The refusal is the same shape as the store's non-POSIX activation: a robot runs Linux (§23.5),
/// and the alternative to failing here is a token derived from whatever a workstation *can* offer —
/// a clock, a process id, an address — which is the weak credential §39.2 says must not be issued.
/// A developer who wanted one anyway would get one that is fine on their machine and catastrophic
/// on the fleet, which is the worst of the available outcomes.
#[cfg(not(unix))]
pub fn random_hex(_length: usize) -> std::io::Result<String> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no /dev/urandom: §39.2 requires a credential to come from the system's entropy source, \
         and this platform is not the one a robot runs",
    ))
}

/// What a route requires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Requirement(pub Role);

impl Requirement {
    /// Whether this role satisfies it.
    pub fn satisfied_by(self, role: Role) -> bool {
        role >= self.0
    }
}

/// The authorisation table, as data.
///
/// A table rather than a check inside each handler, because §39.1's list is short, exact, and the
/// sort of thing that decays one route at a time. Written down once it can be read against the
/// specification, and the suite asserts that the two unauthenticated entries are the only ones.
pub fn requirement(method: &str, segments: &[&str]) -> Requirement {
    let role = match (method, segments) {
        // The two §39.1 gives to nobody in particular.
        ("POST", ["stop"]) => Role::Anonymous,
        ("GET", ["status"]) => Role::Anonymous,

        // The console's own files (§32.2). Answered by the router before this table is consulted,
        // so these arms are unreachable today — and they are here because the catch-all below is
        // `Admin`, and a second caller that asks this table about the console should not be told
        // the page needs an administrator.
        (method, segments) if crate::console::is_asset(method, segments) => Role::Anonymous,

        ("GET", ["health"]) => Role::Observer,
        // Reading the trust store is not changing it. Everything in it is a *public* key and a
        // label the operator chose, so there is nothing to keep from a reader — and the person
        // staring at a package that reports `untrusted` needs to see which keys the robot holds
        // before they can say whether the remedy is a new key or a tampered package. Requiring an
        // admin credential to answer that would send the one diagnosis this store exists to
        // support through a role most people watching a robot do not have.
        ("GET", ["trust"]) => Role::Observer,
        // §10's identity, and the same argument as the line above it. Everything on this route is
        // a public key, a namespace, a device identifier the Hub already knows, and a list of
        // findings — the credential is not on it and never will be. The person who needs it most
        // is the one looking at a robot that is reporting to nobody, and putting that behind an
        // admin credential would gate the diagnosis behind the thing that is broken.
        ("GET", ["identity"]) => Role::Observer,
        // Everything on the check-in route is already on `/health`, `/plan`, `/state` and
        // `/identity`, which are all observer. Putting the assembled version behind a wider role
        // would mean the one view that shows what the Hub is being told is the one an operator
        // watching a robot cannot see.
        ("GET", ["checkin"]) => Role::Observer,
        // §39.1 gives observer "read health, guards, plan". Where the plan's *input* came from is
        // the same class of fact, and it is the one that explains a plan that looks wrong.
        ("GET", ["intent"]) => Role::Observer,
        // §39.1's table, quoted: "Admin — pair and unpair with the Hub". Both, and unpair as much
        // as pair: an operator role that could unpair could take a robot out of a fleet without
        // being able to put it back, which is the destructive half of an admin action handed to
        // somebody the table did not give it to.
        ("POST", ["identity", "pair"]) => Role::Admin,
        ("POST", ["identity", "unpair"]) => Role::Admin,
        ("GET", ["audit"]) => Role::Observer,
        // §39.1 gives observer "read health, guards, plan". Which guards have lost their input is
        // guard state, and it is the panel somebody reads when a robot is running with fewer of
        // them than it should be.
        ("GET", ["calibration"]) => Role::Observer,
        // §55.2's cell. Observer for `/calibration`'s reason and one more: everything on it is what
        // an *operator* wrote into a profile about their own building — a door, a bench, a volume —
        // and the person who needs it is whoever is looking at a skill that was just refused for
        // naming a fact. Putting the explanation behind a wider role than the refusal is how a
        // diagnosis goes unread.
        ("GET", ["cell"]) => Role::Observer,
        // What a robot is waiting to say, and what it has thrown away for want of a link. Both are
        // facts about this machine's health rather than about its contents — the bodies are not
        // served here — so they belong with health and the plan.
        ("GET", ["outbound"]) => Role::Observer,
        ("GET", ["watchdog"]) => Role::Observer,
        // §36's state, and it sits with the watchdog rather than with `GET /status` for the same
        // reason: an unauthenticated caller gets "stop, and see status" (§39.1), and this route
        // carries the reserve, the declared thresholds and which of §36.2's terms nobody supplied.
        // How much energy a robot has left is a diagnosis, and on a fleet it is a movement pattern.
        ("GET", ["power"]) => Role::Observer,
        // §39.1 gives an unauthenticated caller "stop, and see status", and a self-test verdict is
        // not status — it names which of a robot's safety proofs have and have not been made, which
        // is the kind of thing an observer credential exists for.
        ("GET", ["post"]) => Role::Observer,
        // §27's state, and the same reasoning one line up. §39.1's unauthenticated caller gets
        // "stop, and see status", and `GET /status` carries the state word for exactly that
        // reason; this route carries the *reason* and which conditions nobody observed, which is
        // diagnosis rather than status.
        ("GET", ["state"]) => Role::Observer,
        ("GET", ["embodiment"]) => Role::Observer,
        ("GET", ["capacity"]) => Role::Observer,
        ("GET", ["packages"]) => Role::Observer,
        ("GET", ["packages", _]) => Role::Observer,
        ("GET", ["plan"]) => Role::Observer,
        ("GET", ["grants"]) => Role::Observer,
        ("GET", ["logs", _]) => Role::Observer,
        // §39.1 gives observer "the black box index" and operator "seal and download the black
        // box". The index is a count and a window; the file is every command the robot was given.
        ("GET", ["blackbox"]) => Role::Observer,
        // What the robot has been asked to do and how it went. §39.1 gives observer "read health,
        // guards, plan", and a list of invocations is the same kind of fact — it names no
        // arguments a caller supplied and no criterion a publisher wrote, only the ladder.
        // Which image is in which slot, and whether the next one may be installed. §39.1 gives
        // observer "read health, guards, plan", and this is the same kind of fact — it names no
        // key, carries no bundle, and is the panel somebody reads when a fleet has stopped
        // updating.
        ("GET", ["ota"]) => Role::Observer,
        ("GET", ["skills"]) => Role::Observer,
        ("GET", ["skills", _]) => Role::Observer,

        ("POST", ["packages", _, "start"]) => Role::Operator,
        ("POST", ["packages", _, "stop"]) => Role::Operator,
        ("POST", ["grants"]) => Role::Operator,
        ("DELETE", ["grants", ..]) => Role::Operator,
        ("POST", ["packages", _, "rollback"]) => Role::Operator,
        ("POST", ["blackbox", "seal"]) => Role::Operator,
        // Invoking a skill is asking the robot to move, so it sits with starting a package rather
        // than with reading one. §54.4 puts the kernel under it either way — a skill is not an
        // exemption — but the role is what decides who may ask.
        ("POST", ["skills"]) => Role::Operator,
        ("POST", ["skills", _, "stop"]) => Role::Operator,
        ("GET", ["blackbox", _]) => Role::Operator,

        ("POST", ["packages"]) => Role::Admin,
        ("DELETE", ["packages", ..]) => Role::Admin,
        ("POST", ["profile"]) => Role::Admin,

        // Anything unrecognised needs the most. A route added without a table entry then fails
        // closed for everybody but an admin, rather than inheriting whatever the last arm said.
        _ => Role::Admin,
    };
    Requirement(role)
}

/// A `(package, token)` table, grouped for reporting.
pub fn group(pairs: &[(String, String)]) -> BTreeMap<String, Vec<String>> {
    let mut grouped: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (id, token) in pairs {
        grouped.entry(id.clone()).or_default().push(token.clone());
    }
    for tokens in grouped.values_mut() {
        tokens.sort();
        tokens.dedup();
    }
    grouped
}
