## publish crates

#### TODO
- verify the actions.yml and actions/main.rs line up
- use only core logging in the lib
- dry-run and offline dont work together, we should manually allow this case where the version cannot be found on crates.io


#### TODO (later) 
- implement fallback to "latest" version
- use info logs when we have our own action lib
- display all paths releative
- stream the output of async subcommands? when multiple are running that will be an issue though...
- write a nice actions rust library that can parse inputs (check not empty) and even complex values from JSON or YML.
- write a test harness that parses a github action yml with: ... block and presents it to the param parser funciton

#### Done
// this is the tool we want
// https://github.com/kjvalencik/actions/blob/master/run/index.ts

- build a graph with ready nodes (without publish package dependency)
  - dependencies
  - dependants
- write an async loop that does all the work

```bash
yarn upgrade action-get-release --latest
```
