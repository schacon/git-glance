# Git Glance

This is a very simple Git changelog generator.

It does not rely on any specific style of commit messages (such as "conventional commits") and assumes that you're using GitHub pull requests as the main path to feature integration.

It figures out what the commit range is you are trying to generate a changelog for, then gathers all the associated pull request data, then generates tagged summaries via OpenAI.

![preparing the message](https://github.com/schacon/git-glance/assets/70/b93e513a-cd45-4ab4-bec7-44ece96aa2af)

Once all that data is gathered, it will output a markdown based changelog with links to relevant PRs.

![markdown output](https://github.com/schacon/git-glance/assets/70/1541dc29-c748-43f6-8638-f90875d1cd17)

## Requirements

In order to get the PR data, it assumes that you have `gh` cli tool setup and that we can execute it.

It also uses OpenAI to help with classification and summarization. You will need an OpenAI key or it will bail.

```
$ git config --global --add glance.openai.key sk_blahblahblah
```

You can see if these things are set with `git-glance --check`:

![glance check](https://github.com/schacon/git-glance/assets/70/93ac2f2b-83f1-4369-a696-a1052dbf0bd0)

## Warnings

This is horrible, horrible software and it will probably break. I'm not great at Rust and I've done little testing. It works for me, but if you're looking for solid code, this isn't a great place to look. Have fun. :)
