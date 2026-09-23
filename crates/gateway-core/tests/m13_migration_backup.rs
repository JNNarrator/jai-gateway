//! M13 集成测试：迁移前自动备份 DB（D9-T5）。
//!
//! 背景：迁移失败此前是裸 `?` → `main.rs` 的 `.expect(...)` panic 退出，用户只看到一个
//! 闪退的图标，且**没有任何回滚点**。
//!
//! 语义（三条同时满足才备份）：
//! 1. 文件库（`:memory:` 跳过）
//! 2. `user_version > 0`（全新库没有数据可丢，备份只会留垃圾文件）
//! 3. `user_version < MIGRATIONS.len()`（没有待应用的迁移就不备份，避免每次启动都备份）
//!
//! 用例：
//! 1. 内存库 → 不备份
//! 2. 全新文件库 → 不备份，但 12 条迁移全部应用
//! 3. 已是最新版本 → 不备份
//! 4. 落后一个版本 → 产生备份，且备份是**迁移前**状态（user_version = 11）
//! 5. 保留最近 `MIGRATION_BACKUP_KEEP` 份，更老的被清理
//! 6. 备份目录不可写 → 迁移照常进行（备份失败不阻止启动）

use gateway_core::store::migrations::MIGRATIONS;
use gateway_core::store::{self, open_and_migrate, Db};
use std::path::{Path, PathBuf};

// ---------------------------------------------------------------- 夹具

/// 独立的临时目录（每个用例一个，避免相互干扰）。
fn tmp_dir(tag: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "jai-m13-migbak-{tag}-{}-{}",
        std::process::id(),
        rand::random::<u32>()
    ));
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 把库建到「已应用前 `n` 条迁移」的状态（`user_version = n`）。
///
/// 直接用 `MIGRATIONS` 逐条执行，与 `store::migrate` 用同一套 SQL 与事务边界，
/// 但只跑到第 n 条 —— 这样才是真实的「落后一个版本」的库。
fn seed_db_at_version(path: &str, n: usize) {
    let conn = rusqlite::Connection::open(path).unwrap();
    for (idx, (name, sql)) in MIGRATIONS.iter().enumerate().take(n) {
        let version = (idx + 1) as u32;
        conn.execute_batch(&format!(
            "BEGIN;\n{sql}\nPRAGMA user_version = {version};\nCOMMIT;"
        ))
        .unwrap_or_else(|e| panic!("seed migration {name} 失败: {e}"));
    }
    conn.close().unwrap();
}

fn user_version_of(path: &str) -> u32 {
    let conn = rusqlite::Connection::open(path).unwrap();
    let v = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    conn.close().unwrap();
    v
}

fn backups_of(db_path: &Path) -> Vec<PathBuf> {
    store::list_migration_backups(db_path)
}

// ---------------------------------------------------------------- 用例

/// T5-1：内存库不备份（也没有备份目录）。
#[test]
fn memory_db_is_not_backed_up() {
    let conn = open_and_migrate(":memory:").unwrap();
    let v: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v as usize, MIGRATIONS.len(), "内存库仍应跑完全部迁移");
}

/// T5-2：全新文件库不备份 —— 没有数据可丢，备份只会留垃圾文件。
#[test]
fn fresh_file_db_is_not_backed_up() {
    let dir = tmp_dir("fresh");
    let db_path = dir.join("jai.db");
    let p = db_path.to_str().unwrap().to_string();

    let conn = open_and_migrate(&p).unwrap();
    let v: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v as usize, MIGRATIONS.len(), "12 条迁移应全部应用");

    assert!(
        backups_of(&db_path).is_empty(),
        "全新库不应产生备份，实际: {:?}",
        backups_of(&db_path)
    );
    assert!(
        !dir.join(store::BACKUP_DIR).exists(),
        "全新库不应创建备份目录"
    );
}

/// T5-3：已是最新版本 → 不备份（避免每次启动都产生一个备份）。
#[test]
fn up_to_date_db_is_not_backed_up() {
    let dir = tmp_dir("uptodate");
    let db_path = dir.join("jai.db");
    let p = db_path.to_str().unwrap().to_string();

    // 第一次：全新库，全部迁移应用
    drop(open_and_migrate(&p).unwrap());
    // 第二次：没有待应用迁移
    let conn = open_and_migrate(&p).unwrap();
    drop(conn);

    assert!(
        backups_of(&db_path).is_empty(),
        "无待应用迁移时不应备份，实际: {:?}",
        backups_of(&db_path)
    );
}

/// T5-4：落后一个版本 → 产生备份，且备份内容是**迁移前**状态。
#[test]
fn pending_migration_produces_pre_migration_backup() {
    let dir = tmp_dir("pending");
    let db_path = dir.join("jai.db");
    let p = db_path.to_str().unwrap().to_string();

    let pending = MIGRATIONS.len() - 1; // 差最后一条
    seed_db_at_version(&p, pending);
    assert_eq!(user_version_of(&p), pending as u32);

    let conn = open_and_migrate(&p).unwrap();
    let v: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v as usize, MIGRATIONS.len(), "迁移应补齐到最新");
    drop(conn);

    let backups = backups_of(&db_path);
    assert_eq!(backups.len(), 1, "应产生 1 份备份，实际: {backups:?}");

    // 关键断言：备份是迁移**前**的快照，不是迁移后的
    let backup_version = user_version_of(backups[0].to_str().unwrap());
    assert_eq!(
        backup_version, pending as u32,
        "备份必须是迁移前状态（user_version = {pending}），实际 {backup_version}"
    );
    assert_eq!(user_version_of(&p) as usize, MIGRATIONS.len());
}

/// T5-5：滚动清理，只保留最近 `MIGRATION_BACKUP_KEEP` 份。
#[test]
fn old_backups_are_evicted() {
    let dir = tmp_dir("evict");
    let db_path = dir.join("jai.db");
    let p = db_path.to_str().unwrap().to_string();

    let pending = MIGRATIONS.len() - 1;
    seed_db_at_version(&p, pending);

    // 预置 5 份「很老」的备份（时间戳远小于 now_ms）
    let backup_dir = dir.join(store::BACKUP_DIR);
    std::fs::create_dir_all(&backup_dir).unwrap();
    for ts in 1..=5i64 {
        std::fs::write(backup_dir.join(format!("jai.db.{ts}.bak")), b"stale").unwrap();
    }
    assert_eq!(backups_of(&db_path).len(), 5);

    // 触发一次真实备份 → 清理到 KEEP 份
    drop(open_and_migrate(&p).unwrap());

    let left = backups_of(&db_path);
    assert_eq!(
        left.len(),
        store::MIGRATION_BACKUP_KEEP,
        "应只剩 {} 份，实际 {left:?}",
        store::MIGRATION_BACKUP_KEEP
    );
    // 保留的必须是**最新**的（最老的 1/2/3 被删）
    let names: Vec<String> = left
        .iter()
        .map(|p| p.file_name().unwrap().to_string_lossy().into_owned())
        .collect();
    for stale in ["jai.db.1.bak", "jai.db.2.bak", "jai.db.3.bak"] {
        assert!(!names.contains(&stale.to_string()), "{stale} 应被清理");
    }
    assert!(
        names.iter().any(|n| n.starts_with("jai.db.4")),
        "较新的备份应保留: {names:?}"
    );
}

/// T5-6：备份目录不可写 → **迁移照常进行**。
///
/// 决策 D4：备份失败不阻止启动 —— 磁盘满 / 权限这类原因导致应用打不开，
/// 比「没有备份」更糟；迁移本身仍有逐条事务保护。
#[test]
fn backup_failure_does_not_block_migration() {
    let dir = tmp_dir("nobackupdir");
    let db_path = dir.join("jai.db");
    let p = db_path.to_str().unwrap().to_string();

    let pending = MIGRATIONS.len() - 1;
    seed_db_at_version(&p, pending);

    // 把 `backups` 做成**普通文件** → create_dir_all 必然失败
    std::fs::write(dir.join(store::BACKUP_DIR), b"not a dir").unwrap();

    let conn = open_and_migrate(&p).expect("备份失败不应阻止迁移");
    let v: u32 = conn
        .query_row("PRAGMA user_version", [], |r| r.get(0))
        .unwrap();
    assert_eq!(v as usize, MIGRATIONS.len(), "备份失败时迁移仍应补齐到最新");
}

/// T5-补充：`latest_migration_backup` 返回最新一份（迁移失败时用来提示回滚点）。
#[test]
fn latest_backup_is_the_newest() {
    let dir = tmp_dir("latest");
    let db_path = dir.join("jai.db");
    let backup_dir = dir.join(store::BACKUP_DIR);
    std::fs::create_dir_all(&backup_dir).unwrap();
    for ts in [10i64, 300, 20] {
        std::fs::write(backup_dir.join(format!("jai.db.{ts}.bak")), b"x").unwrap();
    }
    let latest = store::latest_migration_backup(&db_path).expect("应有备份");
    assert_eq!(
        latest.file_name().unwrap().to_string_lossy(),
        "jai.db.300.bak",
        "应取时间戳最大的那份"
    );

    // 无备份目录时不 panic，返回 None
    assert!(store::latest_migration_backup(Path::new("/nonexistent/x/jai.db")).is_none());
}

/// T5-补充：非 `jai.db.<digits>.bak` 形态的文件不被误认成备份。
#[test]
fn unrelated_files_are_not_treated_as_backups() {
    let dir = tmp_dir("unrelated");
    let db_path = dir.join("jai.db");
    let backup_dir = dir.join(store::BACKUP_DIR);
    std::fs::create_dir_all(&backup_dir).unwrap();
    for name in [
        "jai.db",
        "jai.db-wal",
        "jai.db.abc.bak",
        "jai-config.123.json",
        "jai.db.12.bak.tmp",
    ] {
        std::fs::write(backup_dir.join(name), b"x").unwrap();
    }
    assert!(
        backups_of(&db_path).is_empty(),
        "无关文件不应被认成备份: {:?}",
        backups_of(&db_path)
    );
}

/// T5-补充：`Db::in_memory()` 不受影响（走 `:memory:` 分支）。
#[test]
fn db_in_memory_still_works() {
    let db = Db::in_memory().unwrap();
    let v = db
        .with(|c| Ok(c.query_row("PRAGMA user_version", [], |r| r.get::<_, u32>(0))?))
        .unwrap();
    assert_eq!(v as usize, MIGRATIONS.len());
}
