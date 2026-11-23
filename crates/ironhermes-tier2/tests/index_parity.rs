//! D-14 index parity: the USearch HNSW index must agree with the brute-force cosine oracle,
//! persist atomically, rebuild from SQLite when its file is corrupt or stale, and reload when
//! another process bumps `meta.index_generation`.

use ironhermes_core::tier2::TIER2_EMBEDDING_DIM;
use ironhermes_tier2::index::{BruteForceIndex, IndexState, UsearchIndex, VectorIndex};
use ironhermes_tier2::store::{NewClaim, Store};

const MODEL: &str = "fastembed:all-MiniLM-L6-v2";

/// Seeded LCG so the suite needs no rand dependency and is reproducible.
struct Lcg(u64);

impl Lcg {
    fn next_f32(&mut self) -> f32 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        ((self.0 >> 33) as f32 / (1u64 << 31) as f32) - 0.5
    }

    fn unit_vec(&mut self) -> Vec<f32> {
        let mut v: Vec<f32> = (0..TIER2_EMBEDDING_DIM).map(|_| self.next_f32()).collect();
        let norm = v.iter().map(|x| x * x).sum::<f32>().sqrt();
        v.iter_mut().for_each(|x| *x /= norm);
        v
    }
}

fn keys(hits: &[(u64, f32)]) -> Vec<u64> {
    hits.iter().map(|h| h.0).collect()
}

fn allow_all(_: u64) -> bool {
    true
}

fn claim(i: usize) -> NewClaim {
    NewClaim {
        text: format!("claim number {i}"),
        kind: "world".into(),
        subject: "user".into(),
        origin: "consolidation".into(),
        confidence: 0.9,
        source_url: None,
        tier1_target: None,
        tier1_ordinal: None,
        session_id: None,
    }
}

/// Insert `n` claims and one embedding each; returns `(claim pk, vector)`.
fn seed(store: &Store, rng: &mut Lcg, n: usize) -> Vec<(i64, Vec<f32>)> {
    (0..n)
        .map(|i| {
            let pk = store.insert_claim(&claim(i)).unwrap().id;
            let v = rng.unit_vec();
            store.upsert_embedding(pk, MODEL, &v).unwrap();
            (pk, v)
        })
        .collect()
}

fn filled(n: usize, rng: &mut Lcg) -> (UsearchIndex, BruteForceIndex, Vec<Vec<f32>>) {
    let mut u = UsearchIndex::new().unwrap();
    let mut b = BruteForceIndex::new();
    let mut all = Vec::new();
    for key in 0..n as u64 {
        let v = rng.unit_vec();
        u.add(key, &v).unwrap();
        b.add(key, &v).unwrap();
        all.push(v);
    }
    (u, b, all)
}

#[test]
fn index_parity_exact_small() {
    let mut rng = Lcg(1);
    let (u, b, _) = filled(100, &mut rng);
    for _ in 0..20 {
        let q = rng.unit_vec();
        let got = u.search(&q, 5, &allow_all).unwrap();
        let want = b.search(&q, 5, &allow_all).unwrap();
        assert_eq!(keys(&got), keys(&want));
    }
}

#[test]
fn index_parity_recall_1000() {
    let mut rng = Lcg(2);
    let (u, b, _) = filled(1000, &mut rng);
    let mut recall = 0.0;
    for _ in 0..50 {
        let q = rng.unit_vec();
        let got = keys(&u.search(&q, 10, &allow_all).unwrap());
        let want = keys(&b.search(&q, 10, &allow_all).unwrap());
        recall += want.iter().filter(|k| got.contains(k)).count() as f64 / 10.0;
    }
    let mean = recall / 50.0;
    assert!(mean >= 0.95, "mean recall {mean} < 0.95");
}

#[test]
fn index_parity_save_load_roundtrip() {
    let mut rng = Lcg(3);
    let (u, _, _) = filled(60, &mut rng);
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("index.usearch");
    u.save(&path).unwrap();
    assert!(path.exists());
    assert!(
        !tmp.path().join("index.usearch.tmp").exists(),
        "save must rename its .tmp file into place"
    );
    let loaded = UsearchIndex::load(&path).unwrap();
    assert_eq!(loaded.size(), u.size());
    for _ in 0..10 {
        let q = rng.unit_vec();
        assert_eq!(
            u.search(&q, 5, &allow_all).unwrap(),
            loaded.search(&q, 5, &allow_all).unwrap()
        );
    }
}

#[test]
fn index_parity_rebuild_from_sqlite() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(&tmp.path().join("t.db")).unwrap();
    let mut rng = Lcg(4);
    let rows = seed(&store, &mut rng, 50);
    let state = IndexState::rebuild_from_store(&store, MODEL).unwrap();
    assert_eq!(state.size(), 50);

    let mut live = UsearchIndex::new().unwrap();
    for (pk, v) in &rows {
        live.add(*pk as u64, v).unwrap();
    }
    for _ in 0..10 {
        let q = rng.unit_vec();
        assert_eq!(
            keys(&state.search(&q, 5, &allow_all).unwrap()),
            keys(&live.search(&q, 5, &allow_all).unwrap())
        );
    }
}

#[test]
fn index_parity_corrupt_file_rebuilds() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(&tmp.path().join("t.db")).unwrap();
    let mut rng = Lcg(5);
    seed(&store, &mut rng, 12);
    let path = tmp.path().join("index.usearch");
    std::fs::write(&path, b"this is definitely not a usearch index").unwrap();

    let state = IndexState::load_or_rebuild(&store, &path, MODEL).unwrap();
    assert_eq!(state.size() as i64, store.count_embeddings(MODEL).unwrap());
    // The rewritten file is now valid.
    assert_eq!(UsearchIndex::load(&path).unwrap().size(), 12);
}

#[test]
fn index_parity_size_mismatch_rebuilds() {
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(&tmp.path().join("t.db")).unwrap();
    let mut rng = Lcg(6);
    seed(&store, &mut rng, 5);
    let path = tmp.path().join("index.usearch");
    let first = IndexState::load_or_rebuild(&store, &path, MODEL).unwrap();
    assert_eq!(first.size(), 5);

    // Three more embeddings land in SQLite without the file being updated.
    seed(&store, &mut rng, 3);
    let second = IndexState::load_or_rebuild(&store, &path, MODEL).unwrap();
    assert_eq!(second.size(), 8);
}

#[test]
fn multiprocess_generation_reload() {
    let tmp = tempfile::tempdir().unwrap();
    let db = tmp.path().join("t.db");
    let path = tmp.path().join("index.usearch");
    let store_a = Store::open(&db).unwrap();
    let store_b = Store::open(&db).unwrap();
    let mut rng = Lcg(7);
    seed(&store_a, &mut rng, 3);
    let mut a = IndexState::load_or_rebuild(&store_a, &path, MODEL).unwrap();
    let mut b = IndexState::load_or_rebuild(&store_b, &path, MODEL).unwrap();
    assert_eq!(b.size(), 3);

    // Process A embeds one more claim and publishes it.
    let pk = store_a.insert_claim(&claim(99)).unwrap().id;
    let v = rng.unit_vec();
    store_a.upsert_embedding(pk, MODEL, &v).unwrap();
    a.add(pk as u64, &v).unwrap();
    a.commit_added(&store_a, &path).unwrap();
    assert!(store_a.index_generation().unwrap() > b.generation());

    // B has not looked yet; its next ensure_fresh must reload and see the new key.
    assert!(!keys(&b.search(&v, 1, &allow_all).unwrap()).contains(&(pk as u64)));
    b.ensure_fresh(&store_b, &path).unwrap();
    assert_eq!(keys(&b.search(&v, 1, &allow_all).unwrap()), vec![pk as u64]);
}

#[test]
fn dim_guard_index_rejects_wrong_length() {
    let mut u = UsearchIndex::new().unwrap();
    let mut b = BruteForceIndex::new();
    assert!(u.add(1, &vec![0.1; 383]).is_err());
    assert!(b.add(1, &vec![0.1; 385]).is_err());
    let tmp = tempfile::tempdir().unwrap();
    let store = Store::open(&tmp.path().join("t.db")).unwrap();
    let pk = store.insert_claim(&claim(0)).unwrap().id;
    assert!(store.upsert_embedding(pk, MODEL, &vec![0.1; 383]).is_err());
    assert_eq!(store.count_embeddings(MODEL).unwrap(), 0);
}
