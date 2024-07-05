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

use openai_api_rs::v1::api::Client;
use openai_api_rs::v1::chat_completion::{self, ChatCompletionRequest};
use openai_api_rs::v1::common::GPT4_O;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::{exit, Stdio};
use std::{fmt::Write, io::Write as ioWrite};

use indicatif::{ProgressBar, ProgressState, ProgressStyle};

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[arg(short, long)]
    release: Option<String>,

    #[arg(short, long)]
    last: Option<String>,

    #[arg(long)]
    check: bool,
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
    updated_at: String,
    merged_at: String,
}

#[derive(Debug)]
struct PrTaggedSummary {
    tag: String,
    summary: String,
    number: String,
    url: String,
}

fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();
    let repo = Repository::open_from_env().unwrap();

    if cli.check {
        check_setup(&repo);
        exit(0)
    }

    // make the dirs we need if they're not there
    std::fs::create_dir_all(repo.path().join("glance/commits"))?;
    std::fs::create_dir_all(repo.path().join("glance/prs"))?;

    // get the commit list
    println!("{}", "Here is what I'm working with:".green());
    // first, get the tip of the branch (or the -r release sha specified)
    let tip = match &cli.release {
        Some(release) => repo.revparse_single(release).unwrap(),
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
    println!("{}", "Summarizing".green());

    let pr_count = pr_list.len();
    let pb = ProgressBar::new(pr_count.try_into().unwrap());
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

    let mut pos = 0;

    // collect all the pr_infos into a vector of pr_summaries
    let pr_summaries: Vec<_> = pr_list
        .values()
        .map(|pr_info| pr_to_tagged_summary(&repo, pr_info))
        // also increment the progress bar
        .inspect(|_| {
            pos += 1;
            pb.set_position(pos);
        })
        .collect();

    pb.finish_with_message("summarized");

    println!(" ");
    println!("{}", "Changelog".green());

    // if there is a tag on the tip commit, show it
    if let Some(release) = &cli.release {
        // get the date
        let commit = repo.revparse_single(release).unwrap();
        let commit = repo.find_commit(commit.id()).unwrap();
        let time = commit.time().seconds();
        let time = chrono::DateTime::<chrono::Utc>::from_timestamp(time, 0);
        if let Some(time) = time {
            // format like "June 3, 2024"
            let time = time.format("%B %e, %Y").to_string();
            println!("**{}** ({})", release, time);
        } else {
            println!("**{}**", release);
        }
    };

    // group the summaries by tag field
    let mut grouped_pr_summaries = HashMap::new();
    for pr_summary in pr_summaries.iter() {
        match pr_summary {
            Ok(pr_summary) => {
                let tag = pr_summary.tag.clone();
                grouped_pr_summaries
                    .entry(tag)
                    .or_insert_with(Vec::new)
                    .push(pr_summary);
            }
            Err(e) => {
                println!("Error: {}", e);
            }
        }
    }

    // print out the summaries by group
    for (tag, pr_summaries) in grouped_pr_summaries.iter() {
        // capitalize the first letter in the tag
        let tag = tag.chars().next().unwrap().to_uppercase().to_string() + &tag[1..];
        println!("\n** {} **", tag.magenta());
        for &pr_summary in pr_summaries {
            println!(
                "* {} [#{}]({})",
                pr_summary.summary,
                pr_summary.number.blue(),
                pr_summary.url
            );
        }
    }

    if !commit_list.is_empty() {
        println!("## {}", "Other".magenta());
    }
    // print out the commits
    for (commit_oid, commit_info) in commit_list.iter() {
        let short_oid = &commit_oid[..6];
        println!("* {} ({})", commit_info.headline, short_oid);
    }

    Ok(())
}

// check `gh` works
// check openai key
fn check_setup(repo: &Repository) {
    let config = repo.config().unwrap();
    let openai_key = config.get_string("glance.openai.key");
    match openai_key {
        Ok(_) => {
            println!("{}", "* OpenAI key found".green());
        }
        Err(_) => {
            println!("{}", "OpenAI key not found".red());
        }
    }

    let mut cmd = std::process::Command::new("gh");
    cmd.args(["auth", "status"]);
    cmd.stderr(Stdio::piped());
    cmd.stdout(Stdio::piped());
    cmd.stdin(Stdio::null());

    let child = cmd.spawn();
    if child.is_err() {
        println!("{}", "* gh not found".red());
        println!(
            "{}",
            "  - please install gh from https://cli.github.com/".blue()
        );
        return;
    }

    let output = child.unwrap().wait_with_output().unwrap();

    if output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let std_both = format!("{} {}", stdout, stderr);
        println!("{}\n\n {}", "* gh auth status good".green(), std_both);
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let stdout = String::from_utf8_lossy(&output.stdout);
        let std_both = format!("{} {}", stdout, stderr);
        println!("{}\n\n {}", "* Failed to run gh".red(), std_both);
    }
}

fn pr_to_tagged_summary(repo: &Repository, pr: &PrInfo) -> Result<PrTaggedSummary, anyhow::Error> {
    let commits = pr
        .commits
        .iter()
        .map(|commit| format!("* {}", commit.headline))
        .collect::<Vec<String>>()
        .join("\n");

    let prompt = format!(
        "You are a senior software developer writing a one line summary and tag for a pull request.
I will give you a Pull Request title, body and a list of the commit messages.
Write me a tag and one line summary for that pull request in the following json format:

```
{{
    'tag': 'feature',
    'summary': 'updated the css to remove all tailwind references'
}}
```

The tag in parenthesis should be one of: feature, bugfix, documentation, test, misc.

Here is the pull request information:

Title: {}
Body: 
{}

Commit Summaries:
{}

Please respond with only the json data of tag and summary",
        pr.title, pr.body, commits,
    );

    let response = get_ai_response(&repo, prompt)?;

    // parse the json
    // we need to strip the ```json\n``` markdown stuff
    let response = response.replace("```json\n", "").replace("```", "");
    let response: serde_json::Value = serde_json::from_str(&response)?;
    let tag = response["tag"].as_str().unwrap();
    let summary = response["summary"].as_str().unwrap();

    Ok(PrTaggedSummary {
        summary: summary.to_string(),
        tag: tag.to_string(),
        number: pr.number.clone(),
        url: pr.url.clone(),
    })
}

fn get_ai_response(repo: &Repository, prompt: String) -> Result<String, anyhow::Error> {
    let config = repo.config()?;

    /*
    let ai_method = match config.get_string("glance.ai") {
        Ok(ai_method) => ai_method,
        Err(_) => bail!("no ai method configured in git config\nuse `git config --add glance.ai [openai,claude,ollama]` to set one\nthen run git config --add glance.openai.key [openai-key]"),
    };
    println!("Using AI method: {}", ai_method);
    */

    let openai_key = config.get_string("glance.openai.key")?;
    let client = Client::new(openai_key);
    let req = ChatCompletionRequest::new(
        GPT4_O.to_string(),
        vec![chat_completion::ChatCompletionMessage {
            role: chat_completion::MessageRole::user,
            content: chat_completion::Content::Text(prompt),
            name: None,
        }],
    );
    let result = client.chat_completion(req)?;
    return Ok(result.choices[0]
        .message
        .content
        .as_ref()
        .unwrap()
        .to_string());
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
                number: pr_info[0]["number"].to_string(),
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
