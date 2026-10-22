

(function(){
 const status=document.getElementById('status');const help=document.getElementById('help');
 function fail(message){status.textContent=message;help.textContent='Откройте приложение внутри Битрикс24. Если сессия заблокирована браузером, проверьте разрешение сторонних cookies для этого сайта.';}
 if(window.self===window.top){fail('Вход возможен только из Битрикс24.');return;}
 if(typeof BX24==='undefined'){fail('Не загружена библиотека Битрикс24.');return;}
 const timer=setTimeout(()=>fail('Не удалось инициализировать Битрикс24.'),15000);
 // installFinish вызывается лишь во время установки, а не при каждом открытии.
 BX24.install(function(){BX24.installFinish();});
 BX24.init(function(){
   clearTimeout(timer);
   const a=BX24.getAuth();
   if(!a||!a.access_token||!a.member_id||!a.domain){fail('Не получены OAuth-данные Битрикс24.');return;}
   BX24.callMethod('user.current',{},async function(result){
     if(result.error()){fail('Битрикс24 не подтвердил текущего сотрудника.');return;}
     status.textContent='Авторизация сотрудника '+(result.data().NAME||result.data().ID)+'…';
     try {
       const resp=await fetch('/auth/bitrix',{method:'POST',credentials:'same-origin',headers:{'Content-Type':'application/json'},body:JSON.stringify({access_token:a.access_token,member_id:a.member_id,domain:a.domain})});
       if(!resp.ok){throw new Error('HTTP '+resp.status);}
       window.location.replace('/');
     }catch(err){fail('Ошибка серверной авторизации: '+String(err));}
   });
 });
})();
