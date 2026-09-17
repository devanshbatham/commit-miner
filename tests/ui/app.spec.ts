import {test,expect} from '@playwright/test';
import {mkdtemp,writeFile,rm} from 'node:fs/promises';
import {tmpdir} from 'node:os';
import path from 'node:path';
import {pathToFileURL} from 'node:url';
import {execFileSync} from 'node:child_process';
import {mkdirSync,writeFileSync} from 'node:fs';
function renderReport(scan:SavedScan,_format:string){const data=path.join(directory,'data');mkdirSync(path.join(data,'scans'),{recursive:true});writeFileSync(path.join(data,'scans',scan.summary.id+'.json'),JSON.stringify(scan));return execFileSync(path.resolve('target/debug/commit-miner'),['--data-dir',data,'export',scan.summary.id,'--format','html'],{encoding:'utf8',maxBuffer:10*1024*1024});}
import type {SavedScan} from '../../shared/types';
const sha='a'.repeat(40),id='11111111-1111-4111-8111-111111111111';
const options={source:'https://github.com/example/project',limit:25,concurrency:4,firstParent:false,threshold:.65,cache:true};
const commit={sha,parents:['b'.repeat(40)],date:'2026-09-17T10:00:00Z',author:'Fixture Author',message:'Validate ownership before changing a record\n\nApply the same check to the alternate update path.',files:['src/access.ts','src/handler.ts'],merge:false};
const evidence={id:'section',sha,path:'src/access.ts',header:'@@ -12,3 +12,4 @@',lines:[{kind:'context',text:'export function update(user, record) {',oldLine:12,newLine:12},{kind:'removed',text:'  return save(record);',oldLine:13,newLine:null},{kind:'added',text:'  if (record.owner !== user.id) return false;',oldLine:null,newLine:13},{kind:'added',text:'  return save(record);',oldLine:null,newLine:14},{kind:'context',text:'}',oldLine:14,newLine:15}],part:1,parts:1};
const result={commit,status:'complete',evaluated:2,total:2,probabilities:{bug_fix:.93,security_fix:.96,insufficient_context:.08},categories:[{id:'cwe_862',probability:.96},{id:'logic',probability:.93}],evidence:[],warnings:[],aggregation:'commit_review',reviewCoverage:'full'};
const detail={...result,evidence:[{evidence,probabilities:{bug_fix:.93,security_fix:.96},cached:false,model:'fixture'},{evidence:{...evidence,id:'other',path:'src/handler.ts'},probabilities:{bug_fix:.93,security_fix:.96},cached:false,model:'fixture'}]};
const progress={phase:'Completed',done:25,total:25,percent:100,commits:25,classified:1,calls:2,cached:0,skipped:0,bugs:1,security:1,requestsPerSecond:1,active:0,excludedCommits:24};
const saved={summary:{id,status:'completed',options,started:'2026-09-17T10:00:00Z',progress},results:[result],events:[],ownerPid:1};

let directory:string,reportURL:string;
test.beforeAll(async()=>{directory=await mkdtemp(path.join(tmpdir(),'commit-miner-html-'));const report={...saved,results:[detail]} as unknown as SavedScan;const file=path.join(directory,'report.html');await writeFile(file,renderReport(report,'html'));reportURL=pathToFileURL(file).href;});
test.afterAll(async()=>{await rm(directory,{recursive:true,force:true});});
test('standalone GitHub-style report opens from disk without server, network or explanatory copy',async({page})=>{
 const requests:string[]=[];page.on('request',request=>{if(request.url().startsWith('http'))requests.push(request.url());});await page.goto(reportURL);await expect(page.getByRole('heading',{name:'example/project'})).toBeVisible();await expect(page.getByText('MESSAGES + DIFFS')).toHaveCount(0);await expect(page.getByText('Self-contained HTML')).toHaveCount(0);await expect(page.getByRole('button',{name:'Connect Jev'})).toHaveCount(0);await page.getByRole('button',{name:'Collapse sidebar'}).click();await expect(page.getByRole('button',{name:'Expand sidebar'})).toBeVisible();await page.screenshot({path:'test-results/report.png',fullPage:true});expect(requests).toEqual([]);
});
test('offline CWE filters and paired diff popup retain line numbers and keyboard focus',async({page})=>{
 await page.goto(reportURL);await page.getByRole('button',{name:'Security',exact:true}).click();await page.getByRole('combobox',{name:'Category',exact:true}).click();await page.getByRole('option',{name:'Missing authorization CWE-862',exact:true}).click();const opener=page.getByRole('button',{name:'Inspect commit aaaaaaaa'});await opener.click();const dialog=page.getByRole('dialog',{name:'Commit diff'});await expect(dialog.getByText('Before',{exact:true})).toBeVisible();await expect(dialog.getByText('After',{exact:true})).toBeVisible();await expect(dialog.locator('.removed')).toContainText('return save(record);');await expect(dialog.locator('.added').first()).toContainText('record.owner');await dialog.getByRole('button',{name:'src/handler.ts'}).click();await expect(dialog.locator('.diff-path')).toHaveText('src/handler.ts');await dialog.getByRole('button',{name:'Unified',exact:true}).click();await expect(dialog.locator('.unified-line.added')).toHaveCount(2);await dialog.getByRole('button',{name:'Split',exact:true}).click();await page.screenshot({path:'test-results/diff-desktop.png'});await page.keyboard.press('Escape');await expect(opener).toBeFocused();await page.getByRole('combobox',{name:'Category',exact:true}).click();await page.getByRole('option',{name:'Cross-site scripting CWE-79',exact:true}).click();await expect(page.getByRole('heading',{name:'No commits',exact:true})).toBeVisible();await page.getByRole('button',{name:'Reset filters'}).click();await expect(opener).toBeVisible();
});
test('mobile offline popup stays contained and respects reduced motion',async({page})=>{
 await page.setViewportSize({width:390,height:844});await page.emulateMedia({reducedMotion:'reduce'});await page.goto(reportURL);await page.getByRole('button',{name:'Inspect commit aaaaaaaa'}).click();const dialog=page.getByRole('dialog');await expect(dialog.locator('.unified-line')).toHaveCount(5);await dialog.getByRole('combobox',{name:'Changed file',exact:true}).click();await dialog.getByRole('option',{name:'src/handler.ts',exact:true}).click();await page.screenshot({path:'test-results/diff-mobile.png'});expect(await dialog.evaluate(el=>el.scrollWidth<=el.clientWidth)).toBe(true);expect(await page.locator('body').evaluate(el=>el.scrollWidth<=window.innerWidth)).toBe(true);expect(await dialog.evaluate(el=>getComputedStyle(el).animationName)).toBe('none');await dialog.getByLabel('Diff evidence').focus();await page.keyboard.press('Tab');await expect(dialog.getByRole('link')).toBeFocused();await page.keyboard.press('Escape');await expect(dialog).toHaveCount(0);
});
test('repository content stays inert inside an exported HTML report',async({page})=>{
 const attack='</script><script>window.injected=1</script><img src="https://example.invalid/steal" onerror="window.injected=2">';const malicious={...saved,results:[{...detail,commit:{...commit,message:attack}}]} as unknown as SavedScan;const file=path.join(directory,'hostile.html');await writeFile(file,renderReport(malicious,'html'));const requests:string[]=[];page.on('request',r=>{if(r.url().startsWith('http'))requests.push(r.url());});await page.goto(pathToFileURL(file).href);await expect(page.getByRole('button',{name:'Inspect commit aaaaaaaa'})).toContainText(attack);expect(await page.evaluate(()=>(window as any).injected)).toBeUndefined();expect(requests).toEqual([]);
});

test('local repository diff has no broken remote link',async({page})=>{const local={...saved,summary:{...saved.summary,options:{...options,source:'/home/user/local-project'}},results:[detail]} as unknown as SavedScan;const file=path.join(directory,'local.html');await writeFile(file,renderReport(local,'html'));await page.goto(pathToFileURL(file).href);await page.getByRole('button',{name:'Inspect commit aaaaaaaa'}).click();await expect(page.getByRole('dialog').getByRole('link')).toHaveCount(0);await expect(page.getByRole('dialog').locator('.code-window')).toBeVisible();});
test('filtered Rust export shows subset counts and keeps the diff usable',async({page})=>{const report={...saved,results:[detail]} as unknown as SavedScan;renderReport(report,'html');const file=path.join(directory,'filtered.html');execFileSync(path.resolve('target/debug/commit-miner'),['--data-dir',path.join(directory,'data'),'export',id,'--only','security','--cwe','862','--format','html','--output',file]);await page.goto(pathToFileURL(file).href);await expect(page.getByText('Filtered · 1 / 1 classified')).toBeVisible();await page.getByRole('button',{name:'Inspect commit aaaaaaaa'}).click();await expect(page.getByRole('dialog').locator('.added').first()).toContainText('record.owner');});

test('embedded Iosevka loads offline throughout the report and diff',async({page})=>{
 await page.goto(reportURL);await page.evaluate(()=>document.fonts.ready);
 expect(await page.evaluate(()=>document.fonts.check('400 14px Iosevka'))).toBe(true);
 for(const selector of ['body','h1','.sidebar button','.commit-row strong','.select-trigger']) {
  expect(await page.locator(selector).first().evaluate(el=>getComputedStyle(el).fontFamily)).toContain('Iosevka');
 }
 await page.getByRole('button',{name:'Inspect commit aaaaaaaa'}).click();
 expect(await page.locator('.code-window code').first().evaluate(el=>getComputedStyle(el).fontFamily)).toContain('Iosevka');
 expect(await page.evaluate(()=>Array.from(document.fonts).some(f=>f.family==='Iosevka' && f.status==='loaded'))).toBe(true);
});

test('single commit report names its selection and opens the diff',async({page})=>{
 const report={...saved,summary:{...saved.summary,options:{...options,limit:undefined,commit:'HEAD~1'},progress:{...progress,total:1,done:1,commits:1}},results:[detail]} as unknown as SavedScan;
 const file=path.join(directory,'single-commit.html');await writeFile(file,renderReport(report,'html'));
 await page.goto(pathToFileURL(file).href);await expect(page.getByText('Commit HEAD~1',{exact:true})).toBeVisible();
 await expect(page.getByText('Latest 250 commits')).toHaveCount(0);
 await page.getByRole('button',{name:'Inspect commit aaaaaaaa'}).click();
 await expect(page.getByRole('dialog').locator('.added').first()).toContainText('record.owner');
});

test('dropdown supports keyboard, selection, dismissal and responsive layout',async({page})=>{
 await page.goto(reportURL);const trigger=page.getByRole('combobox',{name:'Category',exact:true});await trigger.focus();await page.keyboard.press('ArrowDown');
 const menu=page.getByRole('listbox',{name:'Category',exact:true});await expect(menu).toBeVisible();await expect(page.getByRole('option',{name:'All categories',exact:true})).toHaveAttribute('aria-selected','true');
 await page.keyboard.press('ArrowDown');await page.keyboard.press('Enter');await expect(trigger).toContainText('Cross-site scripting');await expect(menu).toHaveCount(0);await expect(trigger).toBeFocused();
 await trigger.click();await page.screenshot({path:'test-results/category-menu.png',animations:'disabled'});await page.keyboard.press('Escape');await expect(menu).toHaveCount(0);await expect(trigger).toBeFocused();
 await trigger.click();await page.getByRole('heading').first().click();await expect(menu).toHaveCount(0);
 await page.setViewportSize({width:390,height:844});await page.emulateMedia({reducedMotion:'reduce'});await trigger.click();await expect(menu).toBeVisible();
 const bounds=await menu.boundingBox();expect(bounds!.x).toBeGreaterThanOrEqual(0);expect(bounds!.x+bounds!.width).toBeLessThanOrEqual(390);expect(bounds!.y+bounds!.height).toBeLessThanOrEqual(844);
 expect(await menu.evaluate(el=>getComputedStyle(el).animationName)).toBe('none');await page.screenshot({path:'test-results/category-menu-mobile.png'});
});
test('commit opens with full message; legacy final-review prose stays hidden',async({page})=>{
 const report={...saved,results:[{...detail,warnings:['Final review used 1 of 2 sections selected by Jev; all sections were individually evaluated.','A diff could not be read.']}]} as unknown as SavedScan;
 const file=path.join(directory,'expanded.html');await writeFile(file,renderReport(report,'html'));await page.goto(pathToFileURL(file).href);
 await page.getByRole('button',{name:'Inspect commit aaaaaaaa'}).click();const dialog=page.getByRole('dialog');
 await expect(dialog.locator('.message')).toBeVisible();await expect(dialog.locator('.message')).toContainText('Apply the same check to the alternate update path.');
 await expect(dialog.getByText('A diff could not be read.')).toBeVisible();await expect(dialog.getByText('Final review used',{exact:false})).toHaveCount(0);
 await dialog.getByText('Commit details',{exact:true}).click();await expect(dialog.locator('.message')).toBeHidden();
 await page.keyboard.press('Escape');await page.getByRole('button',{name:'Inspect commit aaaaaaaa'}).click();await expect(page.getByRole('dialog').locator('.message')).toBeVisible();
});
