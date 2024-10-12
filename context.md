`work in progress`

# Context for the Proof of Concept eTIMEbank implementation of ecash


#### Contents:
- Introduction
- What is time banking?
- Addressing with the 'how do you value one hour' question
- An experiment in new forms of collectively ascibing 'value'
- Why not just use Bitcoin?
- Relevant features of Chaumian ecash for time banking
- Use-case scenarios
- Design choices for project
- FAQ


## Introduction


Concept is the creation of a FOSS ecash based time-banking application that runs both the wallet client and mint logic for offline and offgrid communities.

This project by design not include any Bitcoin elements and does not use satoshis as a unit, it is an exercise in applying a Chaumian blinded signatures scheme to the concept of Time Banking

To know more about the design choices, what time banking is, and the reason why I made this project please refer to the context.md document within this repository





## What is time banking?







## Addressing with the 'how do you value one hour' question






## An experiment in new forms of collectively ascibing 'value'




## Why not just use Bitcoin?






## Relevant features of Chaumian ecash for time banking




## Use-case scenarios




## Design choices for project

- 100% offline use (no http/API)
- should run on smartphones made after 2010 (ideally a PWA)
- access-control to the mint (not open to anyone)
- etime notes represent minutes of real time (time counter mechanism)
- there is no 'withdrawl' or 'deposit' logic (interal community accounting for time)
- ability to cryptographically query the mint to release enote balances (more transparency with verifiability)


## FAQ
