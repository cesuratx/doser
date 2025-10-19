# Doser Documentation

Welcome to the Doser documentation! This directory contains comprehensive guides for using, developing, and operating the dosing system.

## 📚 Documentation Structure

```
docs/
├── INDEX.md                    # This file - documentation hub
│
├── guides/                     # Learning and development guides
│   ├── DeveloperHandbook.md    # Complete developer guide
│   ├── RUST_PRIMER.md          # Rust introduction
│   ├── RUST_PRIMER_DETAILED.md # Detailed Rust concepts
│   └── Glossary.md             # Terms and definitions
│
├── architecture/               # System design and architecture
│   ├── Overview.md             # High-level architecture
│   ├── Modules.md              # Module organization
│   ├── DataFlow.md             # Data flow diagrams
│   └── ARCHITECTURE.md         # Complete architecture doc
│
├── adr/                        # Architecture Decision Records
│   └── ADR-001-predictive-stop.md
│
├── concepts/                   # Core implementation concepts
│   ├── hardware-abstraction.md # Hardware abstraction layer
│   ├── control-loop.md         # Control loop details
│   ├── fixed-point-filters.md  # Fixed-point arithmetic
│   ├── error-handling.md       # Error handling patterns
│   ├── concurrency.md          # Concurrency model
│   ├── time.md                 # Time handling
│   ├── config.md               # Configuration system
│   ├── logging-jsonl.md        # Logging and telemetry
│   ├── traits-generics.md      # Traits and generics
│   ├── ownership-borrowing.md  # Rust ownership
│   ├── unsafe-os.md            # Unsafe code and OS calls
│   └── [more concept docs...]
│
├── ops/                        # Operations and deployment
│   └── Runbook.md              # Production operations guide
│
├── reference/                  # Reference documentation
│   ├── CONFIG_SCHEMA.md        # Configuration reference
│   ├── OPERATIONS.md           # Operations reference
│   └── PI_SMOKE.md             # Raspberry Pi smoke tests
│
├── testing/                    # Testing strategy
│   └── Strategy.md             # Testing approach
│
└── reviews/                    # Audits and analysis
    ├── security-performance-review.md        # Security audit
    ├── business-best-practices-review.md     # Business review
    ├── fix-1.2-sampler-thread-lifecycle.md  # Thread fix doc
    └── performance-roadmap.md                # Optimization roadmap
```

## 🎯 Quick Navigation

### I want to...

**Get started quickly**
→ [../README.md](../README.md#quick-start)

**Understand the architecture**
→ [architecture/ARCHITECTURE.md](./architecture/ARCHITECTURE.md) → [Overview](./architecture/Overview.md) → [Data Flow](./architecture/DataFlow.md)

**Set up for development**
→ [../CONTRIBUTING.md](../CONTRIBUTING.md) → [Developer Handbook](./guides/DeveloperHandbook.md)

**Learn Rust concepts**
→ [Rust Primer](./guides/RUST_PRIMER.md) → [Detailed Primer](./guides/RUST_PRIMER_DETAILED.md) → [Concepts](./concepts/)

**Deploy to production**
→ [Operations Runbook](./ops/Runbook.md) → [Config Schema](./reference/CONFIG_SCHEMA.md)

**Configure the system**
→ [Config Schema](./reference/CONFIG_SCHEMA.md) → [Config Concept](./concepts/config.md)

**Review security/performance**
→ [Security Review](./reviews/security-performance-review.md) → [Business Review](./reviews/business-best-practices-review.md)

**Run tests**
→ [Testing Strategy](./testing/Strategy.md) → [PI Smoke Tests](./reference/PI_SMOKE.md)

**Troubleshoot issues**
→ [Operations Reference](./reference/OPERATIONS.md) → [Runbook](./ops/Runbook.md) → [Error Handling](./concepts/error-handling.md)

**Understand a decision**
→ [ADR Index](./adr/) → Specific decision record

## 📖 Key Documents by Role

### For Users

- [README](../README.md) - Getting started
- [Config Schema](./reference/CONFIG_SCHEMA.md) - Configuration options
- [Operations](./reference/OPERATIONS.md) - Day-to-day operations

### For Developers

- [Developer Handbook](./guides/DeveloperHandbook.md) - Dev setup and workflow
- [Architecture Overview](./architecture/Overview.md) - System design
- [Concepts](./concepts/) - Implementation details
- [Contributing](../CONTRIBUTING.md) - How to contribute

### For Operators

- [Runbook](./ops/Runbook.md) - Production operations
- [Operations Reference](./reference/OPERATIONS.md) - Operations guide
- [PI Smoke Tests](./reference/PI_SMOKE.md) - Hardware testing

### For Architects

- [Architecture Docs](./architecture/) - System architecture
- [ADRs](./adr/) - Design decisions
- [Business Review](./reviews/business-best-practices-review.md) - Strategic analysis

### For Security Reviewers

- [Security Review](./reviews/security-performance-review.md) - Security audit
- [Error Handling](./concepts/error-handling.md) - Error patterns
- [Unsafe Code](./concepts/unsafe-os.md) - Unsafe usage

## 🔍 Document Types

### Guides (guides/)

**Purpose**: Teach and explain  
**Audience**: Learners, new contributors  
**Examples**: Developer Handbook, Rust primers

### Architecture (architecture/)

**Purpose**: Document system design  
**Audience**: Developers, architects  
**Examples**: Module organization, data flow

### Concepts (concepts/)

**Purpose**: Explain implementation details  
**Audience**: Developers working on specific areas  
**Examples**: Control loop, concurrency, fixed-point math

### Reference (reference/)

**Purpose**: Lookup information  
**Audience**: Users, operators, developers  
**Examples**: Config schema, operations commands

### Operations (ops/)

**Purpose**: Production deployment and management  
**Audience**: DevOps, SREs, operators  
**Examples**: Runbook, deployment guides

### Reviews (reviews/)

**Purpose**: Analysis and audits  
**Audience**: Stakeholders, security, management  
**Examples**: Security audit, business review

### ADRs (adr/)

**Purpose**: Record architectural decisions  
**Audience**: Architects, senior developers  
**Examples**: Choice of predictive stop algorithm

## 📝 Documentation Guidelines

When adding new documentation:

1. **Choose the right location**:

   - Tutorial/guide → `guides/`
   - System design → `architecture/`
   - Decision record → `adr/`
   - Implementation detail → `concepts/`
   - Reference info → `reference/`
   - Operations procedure → `ops/`
   - Analysis/audit → `reviews/`

2. **Use clear, descriptive names**: `control-loop.md` not `loop.md`

3. **Add to this index**: Update the relevant section

4. **Follow markdown best practices**:

   - Use proper heading hierarchy (# → ## → ###)
   - Include code examples with syntax highlighting
   - Add diagrams where helpful (mermaid supported)
   - Link to related docs
   - Add a "See also" section for related docs

5. **Keep it up to date**: Update docs when code changes

6. **ADR format**: Use [MADR template](https://adr.github.io/madr/) for new ADRs

## 🔄 Documentation Maintenance

**Last Updated**: October 2025  
**Maintained By**: Doser Contributors  
**Review Cycle**: Quarterly  
**Next Review**: January 2026

### Recent Changes (October 2025)

- ✅ Reorganized documentation structure into logical folders
- ✅ Added INDEX.md as documentation hub
- ✅ Moved ARCHITECTURE.md to architecture/
- ✅ Created reviews/ folder for audits
- ✅ Created guides/ folder for learning materials
- ✅ Created reference/ folder for lookup docs
- ✅ Added business and best practices review
- ✅ Added Phase 1 security fixes documentation

### Upcoming

- [ ] Add troubleshooting guide
- [ ] Add API documentation (generated from rustdoc)
- [ ] Add deployment checklist
- [ ] Add performance tuning guide

---

**Need help?** Open an issue or check [CONTRIBUTING.md](../CONTRIBUTING.md) for how to get support.
