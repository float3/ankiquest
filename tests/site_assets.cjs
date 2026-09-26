const fs=require('node:fs'),path=require('node:path');
const root=path.join(__dirname,'../static');
exports.siteScript=()=> 'window.AnkiQuestSpanish='+fs.readFileSync(path.join(root,'translations-es.json'),'utf8')+';\n'+fs.readFileSync(path.join(root,'i18n.js'),'utf8')+'\n'+fs.readFileSync(path.join(root,'site.js'),'utf8');
