## publish crates

#### TODO
- verify the actions.yml and actions/main.rs line up
- use only core logging in the lib

#### Done
// this is the tool we want
// https://github.com/kjvalencik/actions/blob/master/run/index.ts

- build a graph with ready nodes (without publish package dependency)
  - dependencies
  - dependants
- write an async loop that does all the work
