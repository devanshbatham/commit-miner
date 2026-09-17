import {build} from 'esbuild';
import {rm,mkdir,readFile,writeFile} from 'node:fs/promises';
await rm(new URL('../dist/',import.meta.url),{recursive:true,force:true});
await mkdir(new URL('../dist/',import.meta.url),{recursive:true});
await build({entryPoints:['src/main.tsx'],bundle:true,format:'iife',platform:'browser',target:'es2022',outfile:'dist/report.js',minify:true,jsx:'automatic',external:['/fonts/*'],define:{'process.env.NODE_ENV':'"production"'}});
let css=await readFile('dist/report.css','utf8');
for(const weight of ['regular','medium']){
  const data=await readFile(`public/fonts/iosevka-${weight}.woff2`);
  css=css.replaceAll(`/fonts/iosevka-${weight}.woff2`,`data:font/woff2;base64,${data.toString('base64')}`);
}
await writeFile('dist/report.css',css);
await mkdir('assets',{recursive:true});
for(const file of ['report.js','report.css'])await writeFile(`assets/${file}`,await readFile(`dist/${file}`));
console.log('Built the embedded HTML report viewer. Rebuild the Rust binary to include it.');
