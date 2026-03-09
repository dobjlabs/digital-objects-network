use std::sync::Arc;

use anyhow::{anyhow, Context, Result};
use rocksdb::{IteratorMode, Options, WriteBatch, DB};

use crate::model::{JobStatus, RelayJob};

const JOB_PREFIX: &[u8] = b"job:";
const TX_FINAL_PREFIX: &[u8] = b"txfinal:";

pub enum InsertJobResult {
    Inserted,
    Existing(RelayJob),
}

pub struct Db {
    db: Arc<DB>,
    tx_final_index_lock: std::sync::Mutex<()>,
}

impl Db {
    pub fn connect(path: &str) -> Result<Self> {
        let mut opts = Options::default();
        opts.create_if_missing(true);
        let db = DB::open(&opts, path).with_context(|| format!("open RocksDB at {path}"))?;
        Ok(Self {
            db: Arc::new(db),
            tx_final_index_lock: std::sync::Mutex::new(()),
        })
    }

    pub fn insert_job_idempotent(&self, job: &RelayJob) -> Result<InsertJobResult> {
        let _guard = self.tx_final_index_lock.lock().expect("lock poisoned");
        if let Some(existing) = self.get_job_by_tx_final(&job.tx_final)? {
            return Ok(InsertJobResult::Existing(existing));
        }

        let mut batch = WriteBatch::default();
        batch.put(job_key(&job.job_id), serde_json::to_vec(job)?);
        batch.put(tx_final_key(&job.tx_final), job.job_id.as_bytes());
        self.db.write(batch)?;
        Ok(InsertJobResult::Inserted)
    }

    pub fn put_job(&self, job: &RelayJob) -> Result<()> {
        self.db
            .put(job_key(&job.job_id), serde_json::to_vec(job)?)?;
        Ok(())
    }

    pub fn get_job(&self, job_id: &str) -> Result<Option<RelayJob>> {
        let Some(bytes) = self.db.get(job_key(job_id))? else {
            return Ok(None);
        };
        Ok(Some(serde_json::from_slice(&bytes)?))
    }

    pub fn get_job_by_tx_final(&self, tx_final: &str) -> Result<Option<RelayJob>> {
        let Some(job_id_bytes) = self.db.get(tx_final_key(tx_final))? else {
            return Ok(None);
        };
        let job_id = String::from_utf8(job_id_bytes.to_vec())
            .context("invalid tx_final->job_id mapping bytes")?;
        self.get_job(&job_id)
    }

    pub fn recover_inflight_jobs(&self, now: i64) -> Result<usize> {
        let mut updated = 0usize;
        let mut batch = WriteBatch::default();
        for mut job in self.all_jobs()? {
            let mut changed = false;
            if matches!(job.status, JobStatus::Sending) {
                job.status = JobStatus::Queued;
                changed = true;
            }
            if matches!(job.status, JobStatus::Submitted) && job.tx_hash.is_none() {
                job.status = JobStatus::Queued;
                changed = true;
            }
            if changed {
                job.next_attempt_at = Some(now);
                job.updated_at = now;
                batch.put(job_key(&job.job_id), serde_json::to_vec(&job)?);
                updated += 1;
            }
        }
        if updated > 0 {
            self.db.write(batch)?;
        }
        Ok(updated)
    }

    pub fn next_due_job(&self, now: i64) -> Result<Option<RelayJob>> {
        let mut due: Option<RelayJob> = None;
        for job in self.all_jobs()? {
            if job.status.is_terminal() {
                continue;
            }
            if job.next_attempt_at.is_some_and(|next| next > now) {
                continue;
            }
            match &due {
                None => due = Some(job),
                Some(current) => {
                    let curr_due = current.next_attempt_at.unwrap_or(current.created_at);
                    let next_due = job.next_attempt_at.unwrap_or(job.created_at);
                    if next_due < curr_due
                        || (next_due == curr_due && job.created_at < current.created_at)
                    {
                        due = Some(job);
                    }
                }
            }
        }
        Ok(due)
    }

    pub fn all_jobs(&self) -> Result<Vec<RelayJob>> {
        let mut jobs = Vec::new();
        for entry in self.db.iterator(IteratorMode::Start) {
            let (key, value) = entry?;
            if key.starts_with(JOB_PREFIX) {
                let job = serde_json::from_slice::<RelayJob>(&value)
                    .map_err(|e| anyhow!("decode job record: {e}"))?;
                jobs.push(job);
            }
        }
        Ok(jobs)
    }
}

fn job_key(job_id: &str) -> Vec<u8> {
    [JOB_PREFIX, job_id.as_bytes()].concat()
}

fn tx_final_key(tx_final: &str) -> Vec<u8> {
    [TX_FINAL_PREFIX, tx_final.as_bytes()].concat()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{JobStatus, RelayJob};
    use tempfile::TempDir;

    fn now() -> i64 {
        1_700_000_000
    }

    fn mk_job(id: &str, tx_final: &str, status: JobStatus, next: Option<i64>) -> RelayJob {
        RelayJob {
            job_id: id.to_string(),
            status,
            payload_bytes: vec![1, 2, 3],
            tx_final: tx_final.to_string(),
            state_root_hash: "0x00".to_string(),
            client_ref: None,
            attempt_count: 0,
            tx_hash: None,
            submitted_at: None,
            block_number: None,
            last_error: None,
            next_attempt_at: next,
            created_at: now(),
            updated_at: now(),
        }
    }

    #[test]
    fn idempotent_insert_by_tx_final() {
        let dir = TempDir::new().unwrap();
        let db = Db::connect(dir.path().to_str().unwrap()).unwrap();

        let job = mk_job("job-1", "0xaa", JobStatus::Queued, Some(now()));
        let inserted = db.insert_job_idempotent(&job).unwrap();
        assert!(matches!(inserted, InsertJobResult::Inserted));

        let second = mk_job("job-2", "0xaa", JobStatus::Queued, Some(now()));
        let existing = db.insert_job_idempotent(&second).unwrap();
        match existing {
            InsertJobResult::Existing(found) => assert_eq!(found.job_id, "job-1"),
            InsertJobResult::Inserted => panic!("expected existing"),
        }
    }

    #[test]
    fn recover_sending_to_queued() {
        let dir = TempDir::new().unwrap();
        let db = Db::connect(dir.path().to_str().unwrap()).unwrap();

        let mut sending = mk_job("job-1", "0xaa", JobStatus::Sending, None);
        sending.next_attempt_at = None;
        db.insert_job_idempotent(&sending).unwrap();

        let updated = db.recover_inflight_jobs(now()).unwrap();
        assert_eq!(updated, 1);
        let got = db.get_job("job-1").unwrap().unwrap();
        assert_eq!(got.status, JobStatus::Queued);
        assert_eq!(got.next_attempt_at, Some(now()));
    }

    #[test]
    fn recover_submitted_without_hash_to_queued() {
        let dir = TempDir::new().unwrap();
        let db = Db::connect(dir.path().to_str().unwrap()).unwrap();

        let submitted = mk_job("job-1", "0xaa", JobStatus::Submitted, None);
        db.insert_job_idempotent(&submitted).unwrap();

        let updated = db.recover_inflight_jobs(now()).unwrap();
        assert_eq!(updated, 1);
        let got = db.get_job("job-1").unwrap().unwrap();
        assert_eq!(got.status, JobStatus::Queued);
        assert_eq!(got.next_attempt_at, Some(now()));
    }

    #[test]
    fn picks_next_due_job() {
        let dir = TempDir::new().unwrap();
        let db = Db::connect(dir.path().to_str().unwrap()).unwrap();

        let a = mk_job("job-a", "0xaa", JobStatus::Queued, Some(now() + 5));
        let b = mk_job("job-b", "0xbb", JobStatus::Queued, Some(now()));
        db.insert_job_idempotent(&a).unwrap();
        db.insert_job_idempotent(&b).unwrap();

        let next = db.next_due_job(now()).unwrap().unwrap();
        assert_eq!(next.job_id, "job-b");
    }
}
