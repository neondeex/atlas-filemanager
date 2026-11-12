# Atlas 0.1.6-beta.1: correcciones y validaciÃ³n

Rama de trabajo: `codex/beta-bugfixes-20261112`, basada en el `main` pÃºblico
`38817c6aa117dbf15d8e525f6a95a805b1ef367a`. La copia de trabajo original y sus
cambios de Microsoft Store permanecen en `egui-app`. Las 19 fichas de Partner
Center se guardaron previamente en el borrador del envÃ­o 3.

## Cambios

- Arrastre externo: solo se ofrece COPY. Se eliminÃ³ el evento que iniciaba
  el borrado de originales cuando otra aplicaciÃ³n respondÃ­a MOVE. Los movimientos
  dentro de Atlas siguen perteneciendo al destino interno y a sus modificadores.
- Markdown: se corrigiÃ³ el signo del desplazamiento suave de las listas.
- PDF: los manifiestos antiguos de Document Previews no bloquean el renderizador
  nativo de Windows ni provocan una descarga innecesaria de Poppler.
- Temas: el popover conserva foco, permite pegar/editar hex y HSL/HSLA y no
  descarta la primera ediciÃ³n real tras una actualizaciÃ³n programÃ¡tica.
  El deslizador de tono permite pasos de un grado.
- ConversiÃ³n: sectores opacos, texto blanco/negro segÃºn contraste y selecciÃ³n
  visible inmediatamente, tambiÃ©n con temas personalizados.
- Windows 10: se solicita blur en lugar del material acrylic heredado y se
  inicializa el tamaÃ±o de la estructura usada para consultar la versiÃ³n de Windows.
  Falta la comprobaciÃ³n de rendimiento en un equipo Windows 10 afectado.
- Quick Look: comando `view.quick-look`, con Espacio como valor inicial, editable
  desde Comandos y Ajustes â†’ Atajos. Cambiar/quitar/restaurar se guarda por perfil.
  Se retirÃ³ la asignaciÃ³n estÃ¡tica de Espacio para que no sobreviva a una ediciÃ³n.
- MSIX: framework oficial Desktop VCLibs, servido por Microsoft Store.
- Inno: runtime de escritorio local, avisos completos de rclone y referencia a
  las fuentes correspondientes, ademÃ¡s de los avisos Rust y recursos existentes.

## Estado de reportes

| Reporte | Estado |
| --- | --- |
| Discord: borrar al arrastrar fuera de Atlas | Ruta destructiva eliminada; comprobar tambiÃ©n contra aplicaciones externas reales. |
| Discord: scroll Markdown invertido | Corregido; prueba de regresiÃ³n aÃ±adida en GPUI. |
| Discord: PDF solicita PDF renderer | Corregido; prueba de registro Windows superada. |
| Discord: HOST no admite escritura | La correcciÃ³n que conserva el campo enfocado ya estaba en main/0.1.5; no se sustituyÃ³ por otro arreglo sin evidencia. |
| Discord: se congela y luego cierra al abrir | El reporte no contiene versiÃ³n ni stack. Se necesitan los datos del usuario afectado; no se declara resuelto. |
| GitHub #5: no cambiar tema | Cerrado: el selector separado de Fork Theme ya existe en main y tiene recibos de pruebas de selecciÃ³n/confirmaciÃ³n. |
| GitHub #6: lag al mover con blur en Windows 10 | Cambio de compatibilidad preparado; falta prueba en el entorno afectado. |
| GitHub #7: contraste del selector radial | Corregido en esta rama; no se cerrÃ³ antes de integrar/verificar la correcciÃ³n. |
| GitHub #8: app predeterminada para carpetas | PeticiÃ³n abierta; Win+E es una integraciÃ³n optativa distinta del selector de apps predeterminadas de Windows. |
| GitHub #9: descubrimiento Avahi/WSD | PeticiÃ³n de funciÃ³n abierta. |
| GitHub #13: editor de colores y selector | Selector ya existente; correcciones adicionales de foco/entrada en esta rama. |
| GitHub #14: Kaspersky | Necesita revisiÃ³n del proveedor sobre los binarios exactos. No se desactiva el antivirus ni se afirma aprobaciÃ³n de allowlist. |
| GitHub #15: descarga pÃºblica Linux | DecisiÃ³n de distribuciÃ³n pendiente; esta tarea prepara Windows. |

## CompilaciÃ³n y distribuciÃ³n

La compilación local agotó memoria tanto en release como en release-perf.
Los paquetes se compilan en el runner Windows de GitHub Actions con el perfil
release habitual, conservando la referencia exacta de esta rama.

Ambos formatos utilizan la misma compilaciÃ³n Free con desbloqueo Premium.
La versiÃ³n de aplicaciÃ³n es `0.1.6-beta.1`; la versiÃ³n numÃ©rica MSIX es
`0.1.6.0`. El ZIP de fuentes correspondiente debe acompaÃ±ar los instaladores.
El enlace de una futura release GitHub no se considera publicado por preparar
estos archivos. No se envÃ­a ningÃºn anuncio de Discord en esta tarea.

Antes de la publicaciÃ³n Store siguen pendientes: capturas reales de Windows que
sustituyan las imÃ¡genes rechazadas, instalaciÃ³n limpia/actualizaciÃ³n/desinstalaciÃ³n,
WACK del paquete final y revisiÃ³n de las capacidades y del flujo opcional WinFsp
por Microsoft. Compilar y empaquetar no constituye certificaciÃ³n.
