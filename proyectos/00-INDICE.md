# Proyectos de Infraestructura para Agentes de IA

## Índice de Documentación

| Documento | Descripción |
|-----------|-------------|
| [01-INVESTIGACION-SANDBOX-MCP.md](./01-INVESTIGACION-SANDBOX-MCP.md) | Investigación exhaustiva del estado del arte en ejecución remota de tools para agentes IA |
| [02-ARQUITECTURA-UNIFICADA.md](./02-ARQUITECTURA-UNIFICADA.md) | Diseño de arquitectura del Gateway MCP con sandboxes remotos via gRPC |
| [03-PLAN-IMPLEMENTACION.md](./03-PLAN-IMPLEMENTACION.md) | Plan detallado de implementación con tareas, dependencias y estimaciones |
| [04-ROADMAP.md](./04-ROADMAP.md) | Roadmap completo por fases con hitos, entregables y criterios de aceptación |

---

## Contexto

Esta serie de documentos analiza la viabilidad de construir un **Gateway MCP open-source** que permita
a agentes de IA ejecutar herramientas (tools) en sandboxes remotos aislados, soportando múltiples
backends (Podman, Firecracker, gVisor, Kubernetes) a través de una interfaz unificada.

La investigación se basa en el análisis de plataformas en producción: **E2B**, **Vercel Sandbox**,
**fal-ai Isolate**, **Bolt.new**, **Lovable**, **Jenkins** y **PlanetScale**, y propone un diseño
en Rust utilizando el SDK oficial `rmcp` y `tonic` (gRPC).

## Convenciones

- **Fuentes verificadas** se marcan con el enlace original
- **Correcciones al análisis original** se marcan con ⚠️
- **Descubrimientos nuevos** se marcan con 🔍
- **Recomendaciones** se marcan con ✅
