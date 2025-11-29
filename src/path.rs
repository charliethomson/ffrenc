use std::{path::PathBuf, time::SystemTime};

const PRODUCT_NAME: &str = "dev.thmsn.ffrenc";

fn epoch() -> u64 {
    SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .expect("Why are you in the past?")
        .as_secs()
}

pub fn data_root() -> PathBuf {
    dirs::data_local_dir()
        .expect("cant find data local dir")
        .join(PRODUCT_NAME)
}

pub fn logs_root() -> PathBuf {
    println!("{}", data_root().join("logs").display());
    data_root().join("logs")
}

pub fn logs_path() -> PathBuf {
    let parent = logs_root();
    if !parent.exists() {
        std::fs::create_dir_all(&parent).expect("Failed to create logs root dir");
    }
    parent.join(format!("{}_log.json", epoch()))
}
