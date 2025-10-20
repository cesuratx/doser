# Hardware Abstraction

- Traits `Scale` and `Motor` in `doser_traits` decouple core from GPIO.
- `doser_hardware` provides `SimulatedScale/Motor` and real HX711/stepper under feature flags.
- Pacing utilities (`Pacer`) allow deterministic stepping independent of sensor timing.
