# Git Glance

This is a very simple Git changelog generator.

It does not rely on any specific style of commit messages (such as "conventional commits") and assumes that you're using GitHub pull requests as the main path to feature integration.

In order to get the PR data, it assumes that you have `gh` cli tool setup and that we can execute it. It will work in a basic way without that, but most of the value is associating commits to PRs that have been merged and summarizing them based on that data (PR body, comments, etc).

It also uses AI models to help with classification and summarization. You will need an Anthropic key, OpenAI key or Ollama installation to enable these features.


