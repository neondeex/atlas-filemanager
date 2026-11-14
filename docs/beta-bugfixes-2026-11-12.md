# Atlas 0.1.6-beta.1: correcciones y validación

Rama de trabajo: `codex/beta-bugfixes-20261112`, basada en el `main` público
`38817c6aa117dbf15d8e525f6a95a805b1ef367a`. El código, las correcciones de
Store, los bugfixes y los paquetes se consolidaron en `egui-app`. Los cambios
anteriores se conservan en `backup/local-before-unified-20261112`, un stash y
`artifacts/consolidation-20261112`. Las 19 fichas de Partner Center se guardaron
previamente en el borrador del envío 3.

## Cambios

- Arrastre externo: solo se ofrece COPY. Se eliminó el evento que iniciaba
  el borrado de originales cuando otra aplicación respondía MOVE. Los movimientos
  dentro de Atlas siguen perteneciendo al destino interno y a sus modificadores.
- Markdown: se corrigió el signo del desplazamiento suave de las listas.
- PDF: los manifiestos antiguos de Document Previews no bloquean el renderizador
  nativo de Windows ni provocan una descarga innecesaria de Poppler.
- Temas: el popover conserva foco, permite pegar/editar hex y HSL/HSLA y no
  descarta la primera edición real tras una actualización programática.
  El deslizador de tono permite pasos de un grado.
- Conversión: sectores opacos, texto blanco/negro según contraste y selección
  visible inmediatamente, también con temas personalizados.
- Windows 10: se solicita blur en lugar del material acrylic heredado y se
  inicializa el tamaño de la estructura usada para consultar la versión de Windows.
  Falta la comprobación de rendimiento en un equipo Windows 10 afectado.
- Quick Look: comando `view.quick-look`, con Espacio como valor inicial, editable
  desde Comandos y Ajustes → Atajos. Cambiar/quitar/restaurar se guarda por perfil.
  Se retiró la asignación estática de Espacio para que no sobreviva a una edición.
- MSIX: framework oficial Desktop VCLibs, servido por Microsoft Store.
- Inno: runtime de escritorio local, avisos completos de rclone y referencia a
  las fuentes correspondientes, además de los avisos Rust y recursos existentes.

## Estado de reportes

| Reporte | Estado |
| --- | --- |
| Discord: borrar al arrastrar fuera de Atlas | Ruta destructiva eliminada; comprobar también contra aplicaciones externas reales. |
| Discord: scroll Markdown invertido | Corregido; prueba de regresión añadida en GPUI. |
| Discord: PDF solicita PDF renderer | Corregido; prueba de registro Windows superada. |
| Discord: HOST no admite escritura | La corrección que conserva el campo enfocado ya estaba en main/0.1.5; no se sustituyó por otro arreglo sin evidencia. |
| Discord: se congela y luego cierra al abrir | El reporte no contiene versión ni stack. Se necesitan los datos del usuario afectado; no se declara resuelto. |
| GitHub #5: no cambiar tema | Cerrado: el selector separado de Fork Theme ya existe en main y tiene recibos de pruebas de selección/confirmación. |
| GitHub #6: lag al mover con blur en Windows 10 | Cambio de compatibilidad preparado; falta prueba en el entorno afectado. |
| GitHub #7: contraste del selector radial | Corregido en esta rama; no se cerró antes de integrar/verificar la corrección. |
| GitHub #8: app predeterminada para carpetas | Petición abierta; Win+E es una integración optativa distinta del selector de apps predeterminadas de Windows. |
| GitHub #9: descubrimiento Avahi/WSD | Petición de función abierta. |
| GitHub #13: editor de colores y selector | Selector ya existente; correcciones adicionales de foco/entrada en esta rama. |
| GitHub #14: Kaspersky | Necesita revisión del proveedor sobre los binarios exactos. No se desactiva el antivirus ni se afirma aprobación de allowlist. |
| GitHub #15: descarga pública Linux | Se preparan paquetes para testers; no se publica una release pública. |

## Compilación y distribución

La compilación local agotó memoria. Se utiliza GitHub Actions con el perfil
release habitual. Los ejecutables y las pruebas de Windows y Linux de la revisión
50e9a1a pasaron; el pipeline unificado recompila los paquetes de la revisión final.

MSIX e Inno utilizan los mismos cuatro ejecutables Free con desbloqueo Premium.
Linux usa la misma revisión, versión y edición. El workflow Packages con
`platform=both`, `edition=free` produce las tres salidas sin crear releases.
La versión de aplicación es `0.1.6-beta.1`; la versión MSIX es `0.1.6.0`.
La beta Linux apunta al run de pruebas y no anuncia una release pública ni un
canal AppImage inexistentes. Las fuentes correspondientes acompañan los paquetes.
Los recibos registran versión, revisión, edición y hashes de los archivos.

El EXE se comparte solo como beta temporal. Puede convivir con la versión Store,
pero no la actualiza. Al aprobarse la actualización, se desinstala el EXE y se
vuelve a Store. Las compras Premium de Store requieren la aplicación empaquetada.
No se publica ningún anuncio de Discord ni una release de GitHub automáticamente.

Antes de la publicación Store siguen pendientes: capturas reales de Windows que
sustituyan las imágenes rechazadas, prueba de actualización y lanzamiento GUI del
MSIX final, WACK y revisión de las capacidades y del flujo opcional WinFsp por
Microsoft. Compilar y empaquetar no constituye certificación.
