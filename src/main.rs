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
use git2::Repository;

#[derive(Parser, Debug)]
#[command(version, about, long_about = None)]
struct Cli {
    #[arg(short, long)]
    release: Option<String>,

    #[arg(short, long)]
    last: Option<String>,
}

fn main() -> Result<(), anyhow::Error> {
    let cli = Cli::parse();
    dbg!(&cli);

    println!("Let's get documenting!");

    let repo = Repository::open_from_env().unwrap();
    dbg!(repo.path());

    // get the commit list

    // first, get the tip of the branch (or the -r release sha specified)
    let tip = match cli.release {
        Some(release) => repo.revparse_single(&release).unwrap(),
        None => repo.revparse_single("HEAD").unwrap(),
    };
    dbg!(&tip);

    // then, get the last commit (-l last sha specified or last tag)
    let last = match (cli.last, repo.tag_names(None)?.iter().last()) {
        (Some(sha), _) => repo.revparse_single(&sha).unwrap(),
        (_, Some(Some(last_tag))) => repo.revparse_single(last_tag).unwrap(),
        (_, _) => bail!("no tags found and no last release specified"),
    };
    dbg!(&last);

    Ok(())
}
