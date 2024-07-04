// git-glance
//
// - take the commit range
// - determine all involved commits
// - find related PR information
//   - see if we can group commits into PRs
// - create prompt for each PR/group
// - ask AI for a one line summary
// - determine feature, bug fix, documentation, etc
// - compose release notes and output markdown
//   - optionally with debug information (which commits/pr)

use anyhow::bail;
use clap::Parser;
use colored::Colorize;
use git2::Repository;

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Stdio;
use std::{fmt::Write, io::Write as ioWrite};

use indicatif::{ProgressBar, ProgressState, ProgressStyle};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[arg(short, long)]
    release: Option<String>,

    #[arg(short, long)]
    last: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct CommitInfo {
    oid: String,
    headline: String,
    body: String,
    pr: Option<String>,
}

#[derive(Serialize, Deserialize, Debug)]
struct PrInfo {
    number: String,
    title: String,
    body: String,
    author: String,
    comments: Vec<String>,
    commits: Vec<CommitInfo>,
    url: String,
    // date
    updated_at: String,
    merged_at: String,
}

fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();
    let repo = Repository::open_from_env().unwrap();

    // make the dirs we need if they're not there
    std::fs::create_dir_all(repo.path().join("glance/commits"))?;
    std::fs::create_dir_all(repo.path().join("glance/prs"))?;

    // get the commit list

    println!("{}", "Here is what I'm working with:".green());
    // first, get the tip of the branch (or the -r release sha specified)
    let tip = match cli.release {
        Some(release) => repo.revparse_single(&release).unwrap(),
        None => repo.revparse_single("HEAD").unwrap(),
    };
    println!("Tip commit:  {}", tip.id().to_string().blue());

    // then, get the last commit (-l last sha specified or last tag)
    // TODO: actually order by tag date
    let last = match (cli.last, repo.tag_names(None)?.iter().last()) {
        (Some(sha), _) => repo.revparse_single(&sha).unwrap(),
        (_, Some(Some(last_tag))) => repo.revparse_single(last_tag).unwrap(),
        (_, _) => bail!("no tags found and no last release specified"),
    };
    println!("Last commit: {}", last.id().to_string().blue());

    // get the commit range
    let mut revwalk = repo.revwalk()?;
    revwalk.push(tip.id())?;
    revwalk.hide(last.id())?;

    let commits: Vec<_> = revwalk.collect::<Result<Vec<_>, _>>()?;

    //count the vec
    let count = commits.len();
    println!(
        "Number of commits in release: {}",
        count.to_string().green()
    );

    commits.clone().into_iter().for_each(|commit| {
        let commit = repo.find_commit(commit).unwrap();
        let message = commit.summary().unwrap();
        println!("{} {}", commit.id().to_string().blue(), message);
    });

    println!(" ");
    println!("{}", "Getting PR information for commits".green());

    let pb = ProgressBar::new(count.try_into().unwrap());
    pb.set_style(
        ProgressStyle::with_template(
            "{spinner:.green} [{elapsed_precise}] [{wide_bar:.cyan/blue}] {pos}/{len} ({eta})",
        )
        .unwrap()
        .with_key("eta", |state: &ProgressState, w: &mut dyn Write| {
            write!(w, "{:.1}s", state.eta().as_secs_f64()).unwrap()
        })
        .progress_chars("#>-"),
    );

    // get github PR information

    let mut pr_list = HashMap::new();
    let mut commit_list = HashMap::new();

    let mut pos = 0;
    commits.clone().into_iter().for_each(|commit| {
        pos += 1;
        pb.set_position(pos);
        match get_pr_info(&repo, commit) {
            Ok(pr_info) => match pr_info {
                Some(pr_info) => {
                    pr_list.insert(pr_info.number.clone(), pr_info);
                }
                None => {
                    commit_list.insert(commit.to_string(), get_commit_info(&repo, commit).unwrap());
                }
            },
            Err(e) => {
                println!("Error: {}", e);
            }
        }
    });

    pb.finish_with_message("downloaded");

    println!(" ");
    println!("{}", "Basic message".green());

    // print out the PRs
    for (pr_number, pr_info) in pr_list.iter() {
        println!("{} {}", pr_number.blue(), pr_info.title);
    }
    // print out the commits
    for (commit_oid, commit_info) in commit_list.iter() {
        println!("{} {}", commit_oid.blue(), commit_info.headline);
    }

    Ok(())
}

fn get_commit_info(repo: &Repository, commit: git2::Oid) -> Result<CommitInfo, anyhow::Error> {
    let commit_object = repo.find_commit(commit)?;
    let commit_info = CommitInfo {
        oid: commit.to_string(),
        headline: commit_object.summary().unwrap().to_string(),
        body: commit_object.message().unwrap().to_string(),
        pr: None,
    };
    Ok(commit_info)
}

// look for cached data for this commit oid in .git/glance/commits/[oid].json
// if it exists, return it
// if it doesn't exist, run gh pr list --json --search [oid] --state merged
// and cache the result
fn get_pr_info(repo: &Repository, commit: git2::Oid) -> Result<Option<PrInfo>, anyhow::Error> {
    let commit_path = repo
        .path()
        .join("glance/commits")
        .join(commit.to_string() + ".json");

    let commit_object = repo.find_commit(commit)?;

    if commit_path.exists() {
        let file = std::fs::File::open(commit_path)?;
        let reader = std::io::BufReader::new(file);
        let commit_info: CommitInfo = serde_json::from_reader(reader)?;
        let pr_info = match commit_info.pr {
            Some(pr) => {
                let pr_path = repo.path().join("glance/prs").join(pr + ".json");
                let file = std::fs::File::open(pr_path)?;
                let reader = std::io::BufReader::new(file);
                let pr_info: PrInfo = serde_json::from_reader(reader)?;
                Some(pr_info)
            }
            None => None,
        };
        return Ok(pr_info);
    } else {
        let gh_program = "gh";
        let mut cmd = std::process::Command::new(gh_program);
        cmd.args([
            "pr",
            "list",
            "--json",
            "number,title,author,body,comments,commits,url,updatedAt,mergedAt",
            "--search",
            &commit.to_string(),
            "--state",
            "merged",
        ]);

        cmd.stderr(Stdio::piped());
        cmd.stdout(Stdio::piped());
        cmd.stdin(Stdio::null());

        let child = cmd.spawn().unwrap();
        let output = child.wait_with_output().unwrap();

        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            let pr_info: serde_json::Value = serde_json::from_str(stdout.as_ref())?;

            if pr_info[0] == serde_json::Value::Null {
                return Ok(None);
            }

            let commits = pr_info[0]["commits"]
                .as_array()
                .unwrap()
                .iter()
                .map(|commit| CommitInfo {
                    oid: commit["oid"].as_str().unwrap().to_string(),
                    headline: commit["messageHeadline"].as_str().unwrap().to_string(),
                    body: commit["messageBody"].as_str().unwrap().to_string(),
                    pr: Some(pr_info[0]["number"].to_string()),
                })
                .collect();

            let pr_data = PrInfo {
                number: pr_info[0]["number"].as_str().unwrap().to_string(),
                title: pr_info[0]["title"].as_str().unwrap().to_string(),
                body: pr_info[0]["body"].as_str().unwrap().to_string(),
                author: pr_info[0]["author"]["login"].as_str().unwrap().to_string(),
                updated_at: pr_info[0]["updatedAt"].as_str().unwrap().to_string(),
                merged_at: pr_info[0]["mergedAt"].as_str().unwrap().to_string(),
                commits,
                comments: vec![],
                url: pr_info[0]["url"].as_str().unwrap().to_string(),
            };

            let pr_path = repo
                .path()
                .join("glance/prs")
                .join(pr_info[0]["number"].to_string() + ".json");
            let mut file = std::fs::File::create(pr_path)?;
            file.write_all(serde_json::to_string(&pr_data)?.as_bytes())?;

            let commit_cache = CommitInfo {
                oid: commit.to_string(),
                headline: commit_object.summary().unwrap().to_string(),
                body: commit_object.message().unwrap().to_string(),
                pr: Some(pr_info[0]["number"].to_string()),
            };
            let commit_cache_path = repo
                .path()
                .join("glance/commits")
                .join(commit.to_string() + ".json");
            let mut file = std::fs::File::create(commit_cache_path).unwrap();
            file.write_all(serde_json::to_string(&commit_cache).unwrap().as_bytes())
                .unwrap();

            let commits = pr_info[0]["commits"].as_array();
            match commits {
                Some(commits) => {
                    commits.iter().for_each(|commit| {
                        let commit_cache = CommitInfo {
                            oid: commit["oid"].as_str().unwrap().to_string(),
                            headline: commit["messageHeadline"].as_str().unwrap().to_string(),
                            body: commit["messageBody"].as_str().unwrap().to_string(),
                            pr: Some(pr_info[0]["number"].to_string()),
                        };
                        let commit_cache_path = repo
                            .path()
                            .join("glance/commits")
                            .join(commit["oid"].as_str().unwrap().to_string() + ".json");
                        let mut file = std::fs::File::create(commit_cache_path).unwrap();
                        file.write_all(serde_json::to_string(&commit_cache).unwrap().as_bytes())
                            .unwrap();
                    });
                    return Ok(Some(pr_data));
                }
                None => {
                    // nothing
                    let commit_cache = CommitInfo {
                        oid: commit.to_string(),
                        headline: commit_object.summary().unwrap().to_string(),
                        body: commit_object.message().unwrap().to_string(),
                        pr: None,
                    };
                    let commit_cache_path = repo
                        .path()
                        .join("glance/commits")
                        .join(commit.to_string() + ".json");
                    let mut file = std::fs::File::create(commit_cache_path).unwrap();
                    file.write_all(serde_json::to_string(&commit_cache).unwrap().as_bytes())
                        .unwrap();
                }
            }
        } else {
            let stderr = String::from_utf8_lossy(&output.stderr);
            let stdout = String::from_utf8_lossy(&output.stdout);
            let std_both = format!("{} {}", stdout, stderr);
            bail!("Failed to run gh: {}", std_both);
        }
    }

    Ok(None)
}
