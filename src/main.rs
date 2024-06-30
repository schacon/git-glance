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

fn main() {
    let cli = Cli::parse();
    dbg!(cli);

    println!("Let's get documenting!");

    let repo = Repository::open_from_env().unwrap();
    dbg!(repo.path());
}
