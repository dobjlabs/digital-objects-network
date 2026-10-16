# Welcome to Bitcraft

Bitcraft is a crafting game. Your goal is to scale production of various digital resources, such as `bitcraft::Wood`, `bitcraft::Stone`, and `bitcraft::Iron`, in order to accumulate as many `bitcraft::Bit`s as you can.

Every item in Bitcraft is a "Digital Object," a cryptographically-certified `.dobj` file that lives on your computer. Producing different kinds of Digital Objects involves executing different kinds of computational tasks and recipes, and then generating a cryptographic certificate of having done so. Producing complex Digital Objects, such as a `bitcraft::MiningDrill6`, involves orchestrating complex chains of computations over longer periods of time.

Because Digital Objects are files, they can be easily exchanged between different players and machines; to send someone a Digital Object, you simply send them the file.

## How do I get started?

Left click on a `.dobj` file in the file explorer on the left side of the screen to view details about it. Right click on the file to see a list of available actions that can be performed on it. You can start off by using your `bitcraft::Pocketknife` to gather some wood.

Some actions can only be performed on groups of files. For example, crafting a `bitcraft::WoodenAxe` requires three input objects: one `bitcraft::Wood` and two `bitcraft::Stick`s. You can select multiple objects at once with cmd-, ctrl-, or shift-click. Right clicking on the group selection will then show you the available actions that may be executed on the group.

You can learn more about different kinds of items you can produce, and what sorts of recipes you can execute, by clicking on the "Recipebook" tab at the top of the Bitcraft client.

## What's my goal?

You are building a factory to produce `bitcraft::Bit`s. `bitcraft::Bit`s can be created from various resources:

| Resource | Bits per resource |
| -------- | -------- |
| `bitcraft::Wood`     | 1     |
| `bitcraft::Stone`     | 2     |
| `bitcraft::Iron`     | 5     |

Wood, Stone, and Iron are produced through various activities involving computational work (hashing, zero-knowledge proving, program execution, and more). You can increase your rate of production by crafting higher-level production facilities--such as axes, quarries, and drills, which themselves require these base resources to build--or by orchestrating the computational work more effectively.

The **Total Bit Value** of your inventory at any given time is displayed in the UI.


## Is using [automation, AI, multiple devices, ...] allowed?

Yes. Using any tool or behavior to play Bitcraft is allowed; nothing is considered cheating. Because all of Bitcraft's logic takes place on your computer and a decentralized P2P broadcast network, we couldn't stop you from doing anything even if we wanted to.

Bitcraft's underlying "engine," the Digital Objects Daemon, is highly programmable: it ships with an MCP server and an SDK. You are encouraged to customize your experience of playing Bitcraft in any way that you'd like. You and your agents can also access the source code and docs for Bitcraft and the Digital Objects Network [here]().

## How do I exchange objects with other players?

You can send other players a Digital Object by sending them the file containing the Object.

If you receive a Digital Object, you should be aware that the sender likely still possesses a copy of the file and as a result can still use it. However, once you use, consume, or mutate the Object in any way, all copies of the previous state of the Digital Object file are automatically invalidated, making the Object wholly yours. This is made possible by the privacy-preserving P2P broadcast technologies that Bitcraft is built on. 

For convenience, every Object also has a built in function, `Rekey`, that performs a no-op update on the Object, so that you can invalidate previous states without modifying or consuming the object.

The Digital Objects programming language, Podlang, can also be used to define container Objects that can make exchanges easier and faster--for example, container Objects that hold and "lock" other objects until certain conditions are met, or container Objects that allow for transactional swaps and updates across multiple Objects.

## How does Bitcraft work under the hood? How do I learn more?

Bitcraft is built on Digital Objects technology. Digital Objects are programmable state machines, represented as files, that can be passed between mutually untrusting Internet users while maintaining their integrity and consistency. This is made possible by recent innovations in recursive zero-knowledge proofs ([POD2](https://0xparc.github.io/pod2/)) and decentralized consensus (the Ethereum network's "[public bulletin board](https://x.com/VitalikButerin/status/2032091657819316620?s=20)").

You can learn more about Digital Objects [here](), and you can join the Bitcraft community on Discord [here]().