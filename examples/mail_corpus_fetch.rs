//! Dev tool: fetch message bodies over IMAP into files, for the render corpus
//! (`tools/render-corpus/README.md`). Read-only: `BODY.PEEK[]` never marks a message
//! `\Seen`, and nothing is written to the profile or the message cache. Uses the configured
//! accounts and their stored credentials (the OS may ask to allow this binary to read them).
//!
//! usage: mail_corpus_fetch LIST.tsv OUT_DIR   (LIST rows: id \t uid \t account_id \t folder,
//! as `tools/render-corpus/corpus.py` writes to corpus/uncached.tsv)
use std::collections::BTreeMap;

use cosmic_mail::{accounts, sync::imap};

fn main() -> anyhow::Result<()> {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut args = std::env::args().skip(1);
    let list = std::fs::read_to_string(args.next().expect("LIST.tsv"))?;
    let out = std::path::PathBuf::from(args.next().expect("OUT_DIR"));
    std::fs::create_dir_all(&out)?;

    // account -> folder -> [(id, uid)]
    let mut work: BTreeMap<String, BTreeMap<String, Vec<(i64, u32)>>> = BTreeMap::new();
    for line in list.lines().filter(|l| !l.trim().is_empty()) {
        let f: Vec<&str> = line.split('\t').collect();
        work.entry(f[2].to_string())
            .or_default()
            .entry(f[3].to_string())
            .or_default()
            .push((f[0].parse()?, f[1].parse()?));
    }
    let all = accounts::load_accounts()?;
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(async {
        for (account_id, folders) in work {
            let Some(account) = all.iter().find(|a| a.id == account_id) else {
                eprintln!("no account {account_id}");
                continue;
            };
            let mut session = match imap::connect(account).await {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("connect failed for account {}: {e:#}", &account_id[..8]);
                    continue;
                }
            };
            for (folder, msgs) in folders {
                if let Err(e) = imap::select(&mut session, &folder).await {
                    eprintln!("select {folder} failed: {e:#}");
                    continue;
                }
                for (id, uid) in msgs {
                    match imap::fetch_body(&mut session, uid).await {
                        Ok(body) => {
                            if let Some(html) = body.html.filter(|h| !h.trim().is_empty()) {
                                std::fs::write(out.join(format!("{id}.html")), html).ok();
                                println!("{id}\thtml");
                            } else if let Some(text) = body.text {
                                std::fs::write(out.join(format!("{id}.txt")), text).ok();
                                println!("{id}\ttext");
                            } else {
                                println!("{id}\tempty");
                            }
                        }
                        Err(e) => println!("{id}\terror\t{e:#}"),
                    }
                }
            }
            let _ = session.logout().await;
        }
    });
    Ok(())
}
