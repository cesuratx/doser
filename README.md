# Doser CLI

## Overview

**Doser CLI** is a command-line tool that allows you to manage and control the target dose for a specific application. This project is built using **Rust** and utilizes the **clap** crate for argument parsing.

The tool allows you to set a target dose, calibrate the doser, and more. It's designed to be simple and flexible for testing and configuration purposes.

## Features

- **Set the target dose**: Allows you to set a target dose in grams.
- **Calibrate the doser**: Provides a simple way to calibrate the system.
- **Verbose Mode**: Enable detailed logging of actions with the `--verbose` flag.

## Installation

### Prerequisites

Ensure that you have **Rust** installed on your machine. You can install Rust using the following command:

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
