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

[Wikipedia entry introduction:](https://en.wikipedia.org/wiki/Time_banking) 

```
"In economics, a time-based currency is an alternative currency or exchange system where the unit of account is the person-hour or some other time unit.

Some time-based currencies value everyone's contributions equally: one hour equals one service credit. In these systems, one person volunteers to work for an hour for another person; thus, they are credited with one hour, which they can redeem for an hour of service from another volunteer.

Others use time units that might be fractions of an hour (e.g. minutes, ten minutes – 6 units/hour, or 15 minutes – 4 units/hour). While most time-based exchange systems are service exchanges in that most exchange involves the provision of services that can be measured in a time unit, it is also possible to exchange goods by 'pricing' them in terms of the average national hourly wage rate (e.g. if the average hourly rate is $20/hour, then a commodity valued at $20 in the national currency would be equivalent to 1 hour).  "
```

Time Banking is an alternative currency system with a long history and [active communities that operate with these systems in 2024](https://en.wikipedia.org/wiki/Time-based_currency#Studies_and_examples), it is an alternative to the fiat banking system which uses a unit of account this abstracted from meatspace/reality. 

Similarly to Bitcoin, Time Banking exists in contrasts to the current social consensus that 'time is money' and that a service or product (both of which can be calculated in person-hours) are attributed a numismatic 'value' to it which can be exchange/traded with another party for other services or products, creating an 'economy'

Time Banking presents a unique mental and social model for what 'economy' is an reframes the 'numismatic value' of the time that a person contributes to their local community/society/economy - 'time is money, so let us collaborate with minutes and not fiat'

There has been a plethora of research and books that have been written on the concept of Time Banking, this section is just a small introduction and primer for the understanding of why I chose Time Banking as a alternative type of economy to apply the Cashu protocol and technology within this proof of concept.

For a more in-depth explanation on 'how it works in practice' please see the following webpages:
- [https://timebanking.org/howitworks/](https://timebanking.org/howitworks/)
- [https://timebanks.org/start-a-timebank](https://timebanks.org/start-a-timebank)
- [Video: Time Banking animation](https://www.youtube.com/watch?v=aB8ifVJ34JU)
- [TEDxDouglas: How timebanking can help rebuilding community spirit - Valerie Miller](https://www.youtube.com/watch?v=VRHvoYas82g)
- [TEDxStPeterPort: Timebanking in the UK: It's About Time - Sarah Bird](https://www.youtube.com/watch?v=k0Flh6cuuWs)

**Time Banking Educational Resources**
- [Time Banking wiki](https://wikipedia.org/wiki/Time_banking)
- [Numismatics wiki](https://en.wikipedia.org/wiki/Numismatics)
- [Alternative currency wiki](https://en.wikipedia.org/wiki/Alternative_currency)

**Examples of Time Banks**
- [Custom Gogole Map with Pins for all Time Banking ommunities globally](https://www.google.com/maps/d/viewer?mid=1ZZRA7ombZ7CN_8u8gHIi0wRxq45FaFWs&ll=23.581971987838646%2C2.24820156946123&z=2)
- [Tempo Time Credits](https://wearetempo.org/)
- [Time Banking UK](https://timebanking.org/overview/)
- [bespoke time banking software by Time Banking UK](https://timebanking.org/software/)
- [Time Banking USA](https://timebanks.org/)

**Other Sources**
- [Huffpost](https://www.huffpost.com/entry/bringing-people-together_b_8916374)
- [Vice Portugese - O banco que quer seu tempo, não seu dinheiro](https://www.vice.com/pt/article/o-banco-que-quer-seu-tempo-nao-seu-dinheiro/)
- [Book: Give and Take: How timebanking is transforming healthcare](https://books.google.com/books?id=LIiSBQAAQBAJ)
- [Timebanking (CCIA – 2015)](https://monneta.org/en/timebanking-ccia-2015/)

**Academic papers**
- [Introduction to time banking and Time Credits, 2016](https://www.researchgate.net/publication/297696050_Introduction_to_time_banking_and_Time_Credits)
- [Participação em bancos de tempo: utilizando dados sobre transações para avaliar o Banco de Tempo - Florianópolis](https://www.apec.org.br/rce/index.php/rce/article/view/16)
- [Banco de Tempo-Florianópolis: análise das características socioeconômicas de seus membros](https://ojsrevista.furb.br/ojs/index.php/rbdr/article/view/6937)
- [Time banks: rewarding community self-help in the inner city?](https://academic.oup.com/cdj/article-abstract/39/1/62/268434)



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
