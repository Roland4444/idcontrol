

(function(){
  const nodes=[...document.querySelectorAll('.employee-picker')];
  if(!nodes.length||typeof BX24==='undefined'||window.self===window.top)return;
  function load(){
    BX24.callMethod('user.get',{FILTER:{ACTIVE:true}},function(result){
      if(result.error())return;
      const users=result.data()||[];
      users.forEach(u=>{
        const name=[u.NAME,u.LAST_NAME].filter(Boolean).join(' ')||u.EMAIL||'Сотрудник';
        nodes.forEach(el=>{if(el.querySelector('option[value="'+u.ID+'"]'))return;
          const opt=document.createElement('option');opt.value=String(u.ID);opt.textContent=name+' · ID '+u.ID;el.appendChild(opt);
        });
      });
      if(result.more())result.next();
    });
  }
  BX24.init(load);
  nodes.forEach(select=>select.addEventListener('change',()=>{
    const input=select.closest('form')?.querySelector('input.employee-id');if(input&&select.value)input.value=select.value;
  }));
})();
