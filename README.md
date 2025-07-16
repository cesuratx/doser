# Doser Project

## Overview

**Doser** is a system for managing and controlling dose settings, calibration, and interfacing with hardware. The project consists of several components:

- **Doser CLI**: A command-line interface (CLI) tool for interacting with the system.
- **Doser Core**: The core logic for managing target doses, calibration, and dose adjustments.
- **Doser Hardware**: The hardware interfacing code for controlling devices like motors, sensors, and other peripherals (e.g., GPIO, PWM, I2C).

The Doser project is built using **Rust** for performance, safety, and concurrency. The goal of this project is to provide a customizable solution for controlling and automating dosing systems.

## Features

- **Target Dose Management**: Set and adjust the target dose in grams.
- **Calibration**: Calibrate the system to ensure accurate dosing.
- **Hardware Integration**: Interface with GPIO, PWM, I2C, and other hardware components.
- **CLI Tool**: A user-friendly command-line interface to control the dosing system.
- **Real-time Control**: Allows for immediate adjustment and monitoring of doses.

## Project Structure

The **Doser** project consists of the following components:

### **Doser Core**
The core logic for managing doses, calibration, and other functionality such as error handling and validation.

### **Doser CLI**
A command-line interface that allows users to interact with the dosing system. This interface is built using the **`clap`** crate for argument parsing.

### **Doser Hardware**
The hardware control module that communicates with physical components (motors, sensors, etc.) using protocols such as GPIO, I2C, SPI, and PWM.

## Installation

### Prerequisites

- **Rust**: This project is written in **Rust**, so you'll need to have Rust installed on your machine. You can install Rust by running:

  ```bash
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
